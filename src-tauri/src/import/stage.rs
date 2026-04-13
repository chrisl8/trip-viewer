use crate::error::AppError;
use crate::import::discovery::is_skipped_dir;
use crate::import::diskspace;
use crate::import::hasher;
use crate::import::logger::ImportLogger;
use crate::import::types::{FileEntry, ImportPhase, ImportProgress, ImportSource};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::Emitter;
use walkdir::WalkDir;

/// Minimum interval between progress events (66ms ≈ 15 updates/sec).
const PROGRESS_THROTTLE_MS: u128 = 66;

/// Copy all files from source to `.staging/<label>/`, computing and verifying
/// SHA-256 hashes for each file. Returns the manifest of staged files.
/// On hash mismatch, returns a fatal error — the caller must NOT wipe the source.
pub(crate) fn stage_source(
    source: &ImportSource,
    root_path: &Path,
    cancel_flag: &AtomicBool,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> Result<Vec<FileEntry>, AppError> {
    logger.info(&format!(
        "Phase 1: Staging files from {} ({})",
        source.label, source.path
    ));
    let _ = app.emit("import:phase", super::types::ImportPhaseChange {
        phase: ImportPhase::Staging,
        source_label: source.label.clone(),
        message: format!("Staging files from {} ({})", source.label, source.path),
    });

    let source_path = Path::new(&source.path);

    // Walk source and build manifest
    let mut manifest: Vec<FileEntry> = Vec::new();
    let mut total_size: u64 = 0;

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
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(AppError::Internal("interrupted by user".into()));
        }

        let entry = entry.map_err(|e| AppError::Internal(format!("walk source: {e}")))?;

        if !entry.file_type().is_file() {
            continue;
        }

        // Skip PreAllocFiles — they're dashcam pre-allocated placeholders with
        // no useful content. Excluding them avoids inflating the file count and
        // wasting time copying large empty files. They get deleted during wipe.
        let filename = entry.file_name().to_string_lossy();
        if filename.starts_with(".PreAllocFile_") {
            continue;
        }

        let rel_path = entry
            .path()
            .strip_prefix(source_path)
            .map_err(|e| AppError::Internal(format!("relative path: {e}")))?
            .to_string_lossy()
            .to_string();

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        manifest.push(FileEntry {
            rel_path,
            size,
            source_hash: [0u8; 32],
            staged_path: std::path::PathBuf::new(),
            verified: false,
        });
        total_size += size;
    }

    if manifest.is_empty() {
        logger.info(&format!("No files found in {}", source.path));
        return Ok(manifest);
    }

    logger.info(&format!(
        "Found {} files ({}) in {}",
        manifest.len(),
        diskspace::format_bytes(total_size),
        source.path
    ));

    // Create staging directory and check free space
    let staging_dir = root_path.join(".staging").join(&source.label);
    fs::create_dir_all(&staging_dir)?;

    match diskspace::free_disk_space(&staging_dir) {
        Ok(free) if free < total_size => {
            return Err(AppError::Internal(format!(
                "insufficient disk space: need {} bytes, have {} bytes free",
                total_size, free
            )));
        }
        Err(e) => {
            logger.warn(&format!("Could not check free space: {e}"));
        }
        _ => {}
    }

    // Copy each file with hash verification
    let mut bytes_done: u64 = 0;
    let copy_start = Instant::now();
    let mut last_emit = Instant::now();
    let files_total = manifest.len() as u32;

    for i in 0..manifest.len() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(AppError::Internal("interrupted by user".into()));
        }

        let src_path = source_path.join(&manifest[i].rel_path);
        let dst_path = staging_dir.join(&manifest[i].rel_path);

        // Create parent directories
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Copy with hash
        let (src_hash, written) = hasher::copy_and_hash(&dst_path, &src_path)?;
        manifest[i].source_hash = src_hash;
        manifest[i].staged_path = dst_path.clone();

        logger.info(&format!(
            "Copied {} ({} bytes) sha256={}",
            manifest[i].rel_path,
            written,
            hasher::hash_hex(&src_hash)
        ));

        // Verify destination hash
        let dst_hash = hasher::hash_file(&dst_path)?;
        if src_hash != dst_hash {
            let msg = format!(
                "Hash mismatch after copying file!\n\
                 Source: {}\n\
                 Expected: {}\n\
                 Got:      {}\n\
                 This could indicate a failing SD card, bad USB connection, or disk error.\n\
                 The source has NOT been wiped. Your files are safe.",
                src_path.display(),
                hasher::hash_hex(&src_hash),
                hasher::hash_hex(&dst_hash),
            );
            logger.error(&msg);
            return Err(AppError::Internal(msg));
        }

        manifest[i].verified = true;
        bytes_done += manifest[i].size;

        // Emit throttled progress
        let now = Instant::now();
        let is_last = i + 1 == manifest.len();
        if is_last || now.duration_since(last_emit).as_millis() >= PROGRESS_THROTTLE_MS {
            let elapsed = copy_start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 {
                bytes_done as f64 / elapsed
            } else {
                0.0
            };

            let _ = app.emit("import:progress", ImportProgress {
                phase: ImportPhase::Staging,
                source_label: source.label.clone(),
                files_done: (i + 1) as u32,
                files_total,
                bytes_done,
                bytes_total: total_size,
                current_file: manifest[i].rel_path.clone(),
                speed_bps: speed,
            });
            last_emit = now;
        }
    }

    logger.info(&format!(
        "Staged {} files ({}) from {}",
        manifest.len(),
        diskspace::format_bytes(total_size),
        source.label
    ));

    Ok(manifest)
}
