use crate::error::AppError;
use crate::import::config::ImportConfig;
use crate::import::hasher;
use crate::import::logger::ImportLogger;
use crate::import::types::{
    DistributeResult, FileEntry, ImportPhase, ImportProgress, ImportWarning, UnknownFile,
    UnknownFileAction, UnknownFileDecision,
};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

/// Distribute staged files to Videos/ and Photos/. Collect unknown files.
pub(crate) fn distribute_files(
    manifest: &[FileEntry],
    root_path: &Path,
    config: &ImportConfig,
    cancel_flag: &AtomicBool,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> Result<(DistributeResult, Vec<UnknownFile>), AppError> {
    if manifest.is_empty() {
        return Ok((
            DistributeResult {
                videos_moved: 0,
                photos_moved: 0,
                dups_skipped: 0,
                earliest_date: None,
                latest_date: None,
            },
            Vec::new(),
        ));
    }

    logger.info("Phase 4: Distributing files");
    let _ = app.emit("import:phase", super::types::ImportPhaseChange {
        phase: ImportPhase::Distributing,
        source_label: String::new(),
        message: "Distributing files".to_string(),
    });

    let videos_dir = root_path.join("Videos");
    let photos_dir = root_path.join("Photos");
    fs::create_dir_all(&videos_dir)?;
    fs::create_dir_all(&photos_dir)?;

    let mut result = DistributeResult {
        videos_moved: 0,
        photos_moved: 0,
        dups_skipped: 0,
        earliest_date: None,
        latest_date: None,
    };
    let mut unknowns: Vec<UnknownFile> = Vec::new();
    let total = manifest.len() as u32;

    for (i, entry) in manifest.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(AppError::Internal("interrupted by user".into()));
        }

        let filename = Path::new(&entry.rel_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = Path::new(&filename)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        let _ = app.emit("import:progress", ImportProgress {
            phase: ImportPhase::Distributing,
            source_label: String::new(),
            files_done: (i + 1) as u32,
            files_total: total,
            bytes_done: 0,
            bytes_total: 0,
            current_file: filename.clone(),
            speed_bps: 0.0,
        });

        // PreAllocFile — delete
        if filename.starts_with(".PreAllocFile_") {
            let _ = fs::remove_file(&entry.staged_path);
            logger.info(&format!("Deleted PreAllocFile: {filename}"));
            continue;
        }

        // Ignored file — delete
        if config.is_ignored(&filename) {
            let _ = fs::remove_file(&entry.staged_path);
            logger.info(&format!("Deleted ignored file: {filename}"));
            continue;
        }

        // Classify by extension
        let dest_dir = match ext.as_str() {
            ".mp4" => Some(&videos_dir),
            ".jpg" | ".jpeg" | ".png" => Some(&photos_dir),
            _ => None,
        };

        if let Some(dest_dir) = dest_dir {
            let (moved, is_dup) =
                move_to_destination(entry, dest_dir, app, logger)?;
            if is_dup {
                result.dups_skipped += 1;
            } else if moved {
                match ext.as_str() {
                    ".mp4" => {
                        result.videos_moved += 1;
                        // Track date range from Wolf Box filename: YYYY_MM_DD_HHMMSS_EE_C.MP4
                        if let Some(date) = extract_date_from_filename(&filename) {
                            match &result.earliest_date {
                                None => result.earliest_date = Some(date.clone()),
                                Some(e) if date < *e => result.earliest_date = Some(date.clone()),
                                _ => {}
                            }
                            match &result.latest_date {
                                None => result.latest_date = Some(date.clone()),
                                Some(l) if date > *l => result.latest_date = Some(date.clone()),
                                _ => {}
                            }
                        }
                    }
                    _ => result.photos_moved += 1,
                }
            }
        } else {
            // Unknown file
            unknowns.push(UnknownFile {
                staged_path: entry.staged_path.to_string_lossy().to_string(),
                rel_path: entry.rel_path.clone(),
                extension: ext,
                filename,
                size: entry.size,
            });
        }
    }

    logger.info(&format!(
        "Distributed: {} videos, {} photos, {} duplicates skipped",
        result.videos_moved, result.photos_moved, result.dups_skipped
    ));

    Ok((result, unknowns))
}

/// Move a file to the destination directory, handling collisions and duplicates.
/// Returns (moved, is_duplicate).
fn move_to_destination(
    entry: &FileEntry,
    dest_dir: &Path,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> Result<(bool, bool), AppError> {
    let filename = Path::new(&entry.rel_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let dest_path = dest_dir.join(&filename);

    // No collision — move directly
    if !dest_path.exists() {
        move_file(&entry.staged_path, &dest_path)?;
        logger.info(&format!(
            "Moved {} -> {}",
            entry.rel_path,
            dest_path.display()
        ));
        return Ok((true, false));
    }

    // Collision — check if duplicate
    let existing_hash = hasher::hash_file(&dest_path)?;
    if entry.source_hash == existing_hash {
        // True duplicate
        let _ = fs::remove_file(&entry.staged_path);
        logger.info(&format!("Skipped duplicate: {filename}"));
        return Ok((false, true));
    }

    // Name collision with different content — rename with suffix
    let stem = Path::new(&filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let ext = Path::new(&filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut n = 1u32;
    let renamed_path = loop {
        let candidate = dest_dir.join(format!("{stem}_{n}{ext}"));
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };

    let renamed_name = renamed_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    move_file(&entry.staged_path, &renamed_path)?;

    let msg = format!(
        "Name collision with different content: {filename} — saved as {renamed_name}"
    );
    logger.warn(&msg);
    let _ = app.emit("import:warning", ImportWarning {
        message: msg,
        source_label: String::new(),
    });

    Ok((true, false))
}

/// Move a file, falling back to copy+verify+delete for cross-volume moves.
fn move_file(src: &Path, dst: &Path) -> Result<(), AppError> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }

    // Cross-volume fallback: copy with hash verification
    let (src_hash, _) = hasher::copy_and_hash(dst, src)?;
    let dst_hash = hasher::hash_file(dst)?;
    if src_hash != dst_hash {
        let _ = fs::remove_file(dst);
        return Err(AppError::Internal(
            "hash mismatch in cross-volume move fallback".into(),
        ));
    }
    fs::remove_file(src)?;
    Ok(())
}

/// Apply user decisions for unknown files.
pub(crate) fn apply_unknown_decisions(
    decisions: &[UnknownFileDecision],
    root_path: &Path,
    config: &mut ImportConfig,
    logger: &mut ImportLogger,
) -> Result<u32, AppError> {
    let other_dir = root_path.join("Other");
    let mut count = 0u32;

    for decision in decisions {
        let staged_path = Path::new(&decision.staged_path);
        if !staged_path.exists() {
            continue;
        }

        let filename = staged_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        match decision.action {
            UnknownFileAction::DeleteFilename => {
                let _ = fs::remove_file(staged_path);
                config.add_ignored_filename(&filename, root_path)?;
                logger.info(&format!(
                    "Deleted unknown file and added to ignored filenames: {filename}"
                ));
            }
            UnknownFileAction::DeleteExtension => {
                let ext = Path::new(&filename)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                    .unwrap_or_default();
                let _ = fs::remove_file(staged_path);
                config.add_ignored_extension(&ext, root_path)?;
                logger.info(&format!(
                    "Deleted unknown file and added {ext} to ignored extensions"
                ));
            }
            UnknownFileAction::MoveToOther => {
                fs::create_dir_all(&other_dir)?;
                let dest = other_dir.join(&filename);
                move_file(staged_path, &dest)?;
                logger.info(&format!("Moved unknown file to Other/: {filename}"));
            }
        }

        count += 1;
    }

    Ok(count)
}

/// Extract a date string from a Wolf Box filename like `2026_04_08_163201_00_F.MP4`.
/// Returns `"2026-04-08"` format for easy sorting and display.
fn extract_date_from_filename(filename: &str) -> Option<String> {
    let stem = Path::new(filename)
        .file_stem()?
        .to_string_lossy();
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 3 {
        return None;
    }
    let year = parts[0];
    let month = parts[1];
    let day = parts[2];
    // Validate they look like numbers
    if year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && year.chars().all(|c| c.is_ascii_digit())
        && month.chars().all(|c| c.is_ascii_digit())
        && day.chars().all(|c| c.is_ascii_digit())
    {
        Some(format!("{year}-{month}-{day}"))
    } else {
        None
    }
}
