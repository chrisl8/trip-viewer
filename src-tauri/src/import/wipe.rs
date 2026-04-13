use crate::error::AppError;
use crate::import::discovery::is_skipped_dir;
use crate::import::logger::ImportLogger;
use crate::import::types::{ImportPhase, ImportProgress, ImportSource};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;
use walkdir::WalkDir;

/// Delete all files and directories from the source, skipping system directories.
/// Only called when all files have been verified and the source is not read-only.
pub(crate) fn wipe_source(
    source: &ImportSource,
    cancel_flag: &AtomicBool,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> Result<(), AppError> {
    logger.info(&format!(
        "Phase 3: Wiping source {} ({})",
        source.label, source.path
    ));
    let _ = app.emit("import:phase", super::types::ImportPhaseChange {
        phase: ImportPhase::Wiping,
        source_label: source.label.clone(),
        message: format!("Wiping source {}", source.label),
    });

    if source.read_only {
        let msg = "SD card appears to be read-only. Files were copied but the card will NOT be wiped.";
        logger.warn(msg);
        let _ = app.emit("import:warning", super::types::ImportWarning {
            message: msg.to_string(),
            source_label: source.label.clone(),
        });
        return Ok(());
    }

    let source_path = Path::new(&source.path);

    // Collect files and directories
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();

    for entry in WalkDir::new(source_path)
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
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip root itself
        if entry.path() == source_path {
            continue;
        }

        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        } else if entry.file_type().is_dir() {
            dirs.push(entry.path().to_path_buf());
        }
    }

    let total = files.len() + dirs.len();

    // Delete files
    for (i, file) in files.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(AppError::Internal("interrupted by user".into()));
        }

        if let Err(e) = fs::remove_file(file) {
            logger.warn(&format!("Failed to delete file {}: {e}", file.display()));
        } else {
            logger.info(&format!("Deleted file: {}", file.display()));
        }

        let _ = app.emit("import:progress", ImportProgress {
            phase: ImportPhase::Wiping,
            source_label: source.label.clone(),
            files_done: (i + 1) as u32,
            files_total: total as u32,
            bytes_done: 0,
            bytes_total: 0,
            current_file: file.file_name().unwrap_or_default().to_string_lossy().to_string(),
            speed_bps: 0.0,
        });
    }

    // Delete directories bottom-up (deepest first)
    dirs.sort();
    for dir in dirs.iter().rev() {
        if let Err(e) = fs::remove_dir(dir) {
            logger.warn(&format!(
                "Failed to remove directory {}: {e}",
                dir.display()
            ));
        } else {
            logger.info(&format!("Deleted directory: {}", dir.display()));
        }
    }

    // Verify source is empty (excluding system dirs)
    if let Ok(entries) = fs::read_dir(source_path) {
        let unexpected: Vec<_> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !is_skipped_dir(&name)
            })
            .collect();

        if !unexpected.is_empty() {
            let msg = format!(
                "Source {} has {} remaining entries after wipe",
                source.label,
                unexpected.len()
            );
            logger.warn(&msg);
            let _ = app.emit("import:warning", super::types::ImportWarning {
                message: msg,
                source_label: source.label.clone(),
            });
        }
    }

    logger.info(&format!("Wiped source {}", source.label));
    Ok(())
}
