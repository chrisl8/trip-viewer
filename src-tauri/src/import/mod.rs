pub(crate) mod cleanup;
pub(crate) mod config;
pub(crate) mod discovery;
pub(crate) mod diskspace;
pub(crate) mod distribute;
pub(crate) mod hasher;
pub(crate) mod logger;
pub(crate) mod stage;
pub(crate) mod types;
pub(crate) mod wipe;

use crate::error::AppError;
use config::ImportConfig;
use logger::ImportLogger;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tauri::Emitter;
use types::{
    ImportPhase, ImportPhaseChange, ImportResult, ImportSource, ImportWarning, SourceResult,
    UnknownFileDecision,
};

/// Managed state for the import pipeline.
pub struct ImportState {
    cancel_flag: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    unknown_sender: Arc<Mutex<Option<mpsc::Sender<Vec<UnknownFileDecision>>>>>,
    unknown_receiver: Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
}

impl ImportState {
    pub fn new() -> Self {
        Self {
            cancel_flag: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            unknown_sender: Arc::new(Mutex::new(None)),
            unknown_receiver: Arc::new(Mutex::new(None)),
        }
    }
}

/// Scan for removable drives that look like Wolfbox dashcam SD cards.
#[tauri::command]
pub async fn discover_sources() -> Result<Vec<ImportSource>, AppError> {
    discovery::find_sd_cards()
}

/// Start the import pipeline in a background thread.
#[tauri::command]
pub async fn start_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, ImportState>,
    root_path: String,
    sources: Vec<ImportSource>,
) -> Result<(), AppError> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(AppError::ImportAlreadyRunning);
    }

    // Reset cancel flag
    state.cancel_flag.store(false, Ordering::SeqCst);

    // Set up channel for unknown file decisions
    let (tx, rx) = mpsc::channel::<Vec<UnknownFileDecision>>();
    *state.unknown_sender.lock().map_err(|e| AppError::Internal(e.to_string()))? = Some(tx);
    *state.unknown_receiver.lock().map_err(|e| AppError::Internal(e.to_string()))? = Some(rx);

    let cancel_flag = state.cancel_flag.clone();
    let running = state.running.clone();
    let unknown_receiver = state.unknown_receiver.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let result = run_pipeline(&app, &cancel_flag, &unknown_receiver, &root_path, &sources);
        let _ = app.emit("import:complete", &result);
        running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

/// Cancel a running import.
#[tauri::command]
pub async fn cancel_import(
    state: tauri::State<'_, ImportState>,
) -> Result<(), AppError> {
    if !state.running.load(Ordering::SeqCst) {
        return Err(AppError::NoImportRunning);
    }
    state.cancel_flag.store(true, Ordering::SeqCst);
    Ok(())
}

/// Provide decisions for unknown files, unblocking the pipeline.
#[tauri::command]
pub async fn resolve_unknowns(
    state: tauri::State<'_, ImportState>,
    decisions: Vec<UnknownFileDecision>,
) -> Result<(), AppError> {
    let sender = state
        .unknown_sender
        .lock()
        .map_err(|e| AppError::Internal(format!("lock error: {e}")))?;

    match sender.as_ref() {
        Some(tx) => {
            tx.send(decisions)
                .map_err(|e| AppError::Internal(format!("channel send error: {e}")))?;
            Ok(())
        }
        None => Err(AppError::NoImportRunning),
    }
}

// ── Pipeline orchestration ──

fn run_pipeline(
    app: &tauri::AppHandle,
    cancel_flag: &AtomicBool,
    unknown_receiver: &Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
    root_path: &str,
    sources: &[ImportSource],
) -> ImportResult {
    // The user opens the Videos folder for playback, but the import root is
    // one level up (where Videos/, Photos/, .staging/, .logs/ live as siblings).
    let given = PathBuf::from(root_path);
    let root = if given
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("Videos"))
        .unwrap_or(false)
    {
        given.parent().unwrap_or(&given).to_path_buf()
    } else {
        given
    };
    let mut results: Vec<SourceResult> = Vec::new();

    // Ensure folder structure
    for d in &["Videos", "Photos", ".staging", ".logs"] {
        let _ = fs::create_dir_all(root.join(d));
    }

    // Set up logging
    let logs_dir = root.join(".logs");
    let mut logger = match ImportLogger::new(&logs_dir) {
        Ok(l) => l,
        Err(e) => {
            return error_result(format!("Failed to create logger: {e}"), None);
        }
    };

    logger.info(&format!("Root path: {root_path}"));
    logger.info(&format!("Sources: {}", sources.len()));
    let log_path = Some(logger.path().to_string_lossy().to_string());

    // Rotate old logs
    ImportLogger::rotate(&logs_dir, Duration::from_secs(30 * 24 * 3600));

    // Load import config
    let mut config = ImportConfig::load(&root);

    // Acquire lock
    let lock_path = root.join(".staging").join(".lock");
    if let Err(e) = acquire_lock(&lock_path) {
        logger.error(&format!("Lock error: {e}"));
        return error_result(e.to_string(), log_path);
    }

    // Phase 0: Pre-flight cleanup
    if let Err(e) = cleanup::cleanup_staging(&root, &config, cancel_flag, app, &mut logger) {
        logger.error(&format!("Pre-flight cleanup failed: {e}"));
        let _ = app.emit("import:warning", ImportWarning {
            message: format!("Pre-flight cleanup failed: {e}"),
            source_label: String::new(),
        });
    }

    // Process each source
    for source in sources {
        if cancel_flag.load(Ordering::Relaxed) {
            results.push(cancelled_result(source));
            continue;
        }

        let sr = process_source(
            source,
            &root,
            &mut config,
            cancel_flag,
            unknown_receiver,
            app,
            &mut logger,
        );
        results.push(sr);
        logger.flush();
    }

    // Release lock
    release_lock(&lock_path);
    logger.info("Import complete");

    ImportResult {
        sources: results,
        log_path,
    }
}

fn process_source(
    source: &ImportSource,
    root_path: &Path,
    config: &mut ImportConfig,
    cancel_flag: &AtomicBool,
    unknown_receiver: &Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> SourceResult {
    let mut result = SourceResult {
        source_label: source.label.clone(),
        source_path: source.path.clone(),
        files_staged: 0,
        bytes_staged: 0,
        source_wiped: false,
        read_only: source.read_only,
        videos_moved: 0,
        photos_moved: 0,
        dups_skipped: 0,
        unknown_files: 0,
        no_files: false,
        earliest_date: None,
        latest_date: None,
        error: None,
        warnings: vec![],
    };

    // Phase 1: Stage
    let manifest = match stage::stage_source(source, root_path, cancel_flag, app, logger) {
        Ok(m) => m,
        Err(e) => {
            result.error = Some(e.to_string());
            return result;
        }
    };

    if manifest.is_empty() {
        result.no_files = true;
        return result;
    }

    result.files_staged = manifest.len() as u32;
    result.bytes_staged = manifest.iter().map(|e| e.size).sum();

    // Phase 3: Wipe (only if all verified, not cancelled, not read-only)
    let all_verified = manifest.iter().all(|e| e.verified);
    if all_verified && !cancel_flag.load(Ordering::Relaxed) && !source.read_only {
        match wipe::wipe_source(source, cancel_flag, app, logger) {
            Ok(()) => result.source_wiped = true,
            Err(e) => {
                logger.warn(&format!("Wipe failed: {e}"));
                result.warnings.push(format!("Wipe failed: {e}"));
            }
        }
    } else if source.read_only {
        logger.info("Skipping wipe: source is read-only");
    }

    // Phase 4+5: Distribute
    if !cancel_flag.load(Ordering::Relaxed) {
        match distribute::distribute_files(&manifest, root_path, config, cancel_flag, app, logger) {
            Ok((dr, unknowns)) => {
                result.videos_moved = dr.videos_moved;
                result.photos_moved = dr.photos_moved;
                result.dups_skipped = dr.dups_skipped;
                result.earliest_date = dr.earliest_date;
                result.latest_date = dr.latest_date;

                if !unknowns.is_empty() {
                    result.unknown_files =
                        handle_unknowns(&unknowns, root_path, config, unknown_receiver, app, logger);
                }
            }
            Err(e) => {
                result.error = Some(format!("Distribute failed: {e}"));
                return result;
            }
        }
    }

    // Phase 6: Cleanup
    let _ = app.emit("import:phase", ImportPhaseChange {
        phase: ImportPhase::Cleanup,
        source_label: source.label.clone(),
        message: format!("Cleaning up staging for {}", source.label),
    });
    if let Err(e) = cleanup::cleanup_source(&source.label, root_path, config, logger) {
        logger.warn(&format!("Cleanup error: {e}"));
        result.warnings.push(format!("Cleanup error: {e}"));
    }

    result
}

/// Emit unknowns to frontend and block until decisions arrive via channel.
fn handle_unknowns(
    unknowns: &[types::UnknownFile],
    root_path: &Path,
    config: &mut ImportConfig,
    unknown_receiver: &Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
) -> u32 {
    // Emit unknowns to frontend
    let _ = app.emit("import:unknowns", unknowns);

    // Block until decisions arrive
    let rx_guard = unknown_receiver.lock().ok();
    let decisions = rx_guard
        .as_ref()
        .and_then(|opt| opt.as_ref())
        .and_then(|rx| rx.recv().ok());

    match decisions {
        Some(decisions) => {
            match distribute::apply_unknown_decisions(&decisions, root_path, config, logger) {
                Ok(count) => count,
                Err(e) => {
                    logger.error(&format!("Failed to apply unknown file decisions: {e}"));
                    0
                }
            }
        }
        None => {
            logger.warn("Unknown file channel closed without decisions");
            0
        }
    }
}

fn acquire_lock(path: &Path) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            use std::io::Write;
            let _ = write!(f, "{}", std::process::id());
            Ok(())
        }
        Err(_) => {
            // Lock file exists — check if the owning process is still running.
            // If it's stale (process died), reclaim it.
            if let Ok(contents) = fs::read_to_string(path) {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    if !is_process_alive(pid) {
                        // Stale lock from a crashed process — reclaim it
                        let _ = fs::remove_file(path);
                        return acquire_lock(path);
                    }
                }
            }
            Err(AppError::Internal(format!(
                "Lock file exists at {} — another import may be running",
                path.display()
            )))
        }
    }
}

/// Check if a process with the given PID is still running.
#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::Foundation::CloseHandle;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false; // Can't open = not running (or no permission, which means not ours)
    }
    unsafe { CloseHandle(handle) };
    true
}

#[cfg(not(windows))]
fn is_process_alive(pid: u32) -> bool {
    // Signal 0 tests whether the process exists without actually sending a signal.
    // Works on both Linux and macOS (unlike /proc/{pid} which doesn't exist on macOS).
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn release_lock(path: &Path) {
    if let Err(e) = fs::remove_file(path) {
        eprintln!("Warning: failed to release lock file {}: {e}", path.display());
    }
}

fn error_result(msg: String, log_path: Option<String>) -> ImportResult {
    ImportResult {
        sources: vec![SourceResult {
            source_label: String::new(),
            source_path: String::new(),
            files_staged: 0,
            bytes_staged: 0,
            source_wiped: false,
            read_only: false,
            videos_moved: 0,
            photos_moved: 0,
            dups_skipped: 0,
            unknown_files: 0,
            no_files: false,
            earliest_date: None,
            latest_date: None,
            error: Some(msg),
            warnings: vec![],
        }],
        log_path,
    }
}

fn cancelled_result(source: &ImportSource) -> SourceResult {
    SourceResult {
        source_label: source.label.clone(),
        source_path: source.path.clone(),
        files_staged: 0,
        bytes_staged: 0,
        source_wiped: false,
        read_only: source.read_only,
        videos_moved: 0,
        photos_moved: 0,
        dups_skipped: 0,
        unknown_files: 0,
        no_files: false,
        earliest_date: None,
        latest_date: None,
        error: Some("Cancelled by user".into()),
        warnings: vec![],
    }
}
