use crate::error::AppError;
use crate::import::config::ImportConfig;
use crate::import::discovery::is_skipped_dir;
use crate::import::hasher;
use crate::import::logger::ImportLogger;
use crate::import::types::{FileEntry, ImportPhase};
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tauri::Emitter;
use walkdir::WalkDir;

/// Clean up a specific source's staging directory.
pub(crate) fn cleanup_source(
    source_label: &str,
    root_path: &Path,
    config: &ImportConfig,
    logger: &mut ImportLogger,
) -> Result<(), AppError> {
    logger.info(&format!("Phase 6: Cleaning up staging for {source_label}"));

    let staging_dir = root_path.join(".staging").join(source_label);
    if !staging_dir.exists() {
        return Ok(());
    }

    // Delete PreAllocFiles and ignored files
    for entry in WalkDir::new(&staging_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                !is_skipped_dir(&e.file_name().to_string_lossy())
            } else {
                true
            }
        })
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().to_string();
        if filename.starts_with(".PreAllocFile_") || config.is_ignored(&filename) {
            let _ = fs::remove_file(entry.path());
            logger.info(&format!("Cleanup: deleted {}", entry.path().display()));
        }
    }

    // Remove system directories from staging
    remove_skipped_dirs(&staging_dir);

    // Remove empty directories bottom-up
    remove_empty_dirs(&staging_dir);

    // Try to remove the source label directory itself
    let _ = fs::remove_dir(&staging_dir);

    // Check if parent .staging is empty
    let staging_root = root_path.join(".staging");
    let remaining = count_files(&staging_root);
    if remaining > 0 {
        logger.warn(&format!(
            "Staging has {remaining} files remaining after cleanup"
        ));
    }

    Ok(())
}

/// Pre-flight cleanup: recover leftover files from interrupted runs.
pub(crate) fn cleanup_staging(
    root_path: &Path,
    config: &ImportConfig,
    cancel_flag: &AtomicBool,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> Result<(), AppError> {
    let staging_root = root_path.join(".staging");
    if !staging_root.exists() {
        return Ok(());
    }

    let entries: Vec<_> = match fs::read_dir(&staging_root) {
        Ok(e) => e
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Skip .lock file, only process directories (source labels)
                name != ".lock" && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
            })
            .collect(),
        Err(_) => return Ok(()),
    };

    if entries.is_empty() {
        return Ok(());
    }

    let msg = "Found leftover files in staging from a previous run. Distributing them now.";
    logger.warn(msg);
    let _ = app.emit("import:phase", super::types::ImportPhaseChange {
        phase: ImportPhase::Preflight,
        source_label: String::new(),
        message: msg.to_string(),
    });
    let _ = app.emit("import:warning", super::types::ImportWarning {
        message: msg.to_string(),
        source_label: String::new(),
    });

    for entry in &entries {
        let label = entry.file_name().to_string_lossy().to_string();
        let label_dir = entry.path();

        // Build manifest from staged files
        let manifest = build_manifest_from_staging(&label_dir, logger)?;
        if manifest.is_empty() {
            continue;
        }

        // Distribute them
        let _ = super::distribute::distribute_files(
            &manifest, root_path, config, cancel_flag, app, logger,
        )?;

        // Clean up this label dir
        cleanup_source(&label, root_path, config, logger)?;
    }

    // Verify staging is clean
    let remaining = count_files(&staging_root);
    if remaining > 0 {
        return Err(AppError::Internal(format!(
            "staging directory is not empty after cleanup ({remaining} files remain). \
             Manual intervention required."
        )));
    }

    Ok(())
}

/// Build a manifest from files already in staging (for pre-flight recovery).
fn build_manifest_from_staging(
    dir: &Path,
    logger: &mut ImportLogger,
) -> Result<Vec<FileEntry>, AppError> {
    let mut manifest = Vec::new();

    for entry in WalkDir::new(dir).follow_links(false).into_iter().filter_entry(|e| {
        if e.file_type().is_dir() {
            !is_skipped_dir(&e.file_name().to_string_lossy())
        } else {
            true
        }
    }) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }

        let rel_path = entry
            .path()
            .strip_prefix(dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        let (hash, verified) = match hasher::hash_file(entry.path()) {
            Ok(h) => (h, true),
            Err(e) => {
                logger.warn(&format!(
                    "Could not hash staged file {}: {e}",
                    entry.path().display()
                ));
                ([0u8; 32], false)
            }
        };

        manifest.push(FileEntry {
            rel_path,
            size,
            source_hash: hash,
            staged_path: entry.path().to_path_buf(),
            verified,
        });
    }

    Ok(manifest)
}

/// Recursively remove system directories from the tree.
fn remove_skipped_dirs(root: &Path) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if is_skipped_dir(&name) {
                let _ = fs::remove_dir_all(&path);
            } else {
                remove_skipped_dirs(&path);
            }
        }
    }
}

/// Remove empty directories from bottom up.
fn remove_empty_dirs(root: &Path) {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() && entry.path() != root {
            dirs.push(entry.path().to_path_buf());
        }
    }

    // Sort so deepest paths come last, then reverse
    dirs.sort();
    for dir in dirs.iter().rev() {
        let _ = fs::remove_dir(dir); // only succeeds if empty
    }
}

/// Count non-directory entries under root (excluding .lock).
fn count_files(root: &Path) -> u32 {
    let mut count = 0u32;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                !is_skipped_dir(&e.file_name().to_string_lossy())
            } else {
                true
            }
        })
    {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            if name != ".lock" {
                count += 1;
            }
        }
    }

    count
}
