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
        // SD-card import is destructive on success: wipe_eligible=true.
        let result = run_pipeline(
            &app,
            &cancel_flag,
            &unknown_receiver,
            &root_path,
            &sources,
            true,
        );
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

/// Import all video/photo files from an arbitrary folder into the
/// library. Unlike `start_import`, this is non-destructive — the
/// source folder is left intact (no wipe phase). Use case: re-importing
/// footage that arrived via some non-SD-card route (manual backups,
/// recovery via different tools, files received from someone else,
/// archives discovered after the fact).
///
/// Reuses the SD-card pipeline's stage / distribute / unknowns /
/// cleanup phases verbatim — only discovery and wipe are skipped.
/// Hash-while-copy and verified-destination guarantees still apply.
#[tauri::command]
pub async fn start_folder_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, ImportState>,
    root_path: String,
    source_path: String,
) -> Result<(), AppError> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err(AppError::ImportAlreadyRunning);
    }

    // Resolve library root the same way run_pipeline will, so the
    // self-import guard checks against the actual destination.
    let resolved_root = resolve_library_root(&root_path);

    // Refuse self-import attempts before touching any I/O.
    let source_pb = PathBuf::from(&source_path);
    if let Err(e) = guard_against_self_import(&source_pb, &resolved_root) {
        state.running.store(false, Ordering::SeqCst);
        return Err(e);
    }

    state.cancel_flag.store(false, Ordering::SeqCst);

    let (tx, rx) = mpsc::channel::<Vec<UnknownFileDecision>>();
    *state.unknown_sender.lock().map_err(|e| AppError::Internal(e.to_string()))? = Some(tx);
    *state.unknown_receiver.lock().map_err(|e| AppError::Internal(e.to_string()))? = Some(rx);

    // Synthesize an ImportSource. file_count/total_bytes are populated
    // by the stage phase's own walk; we leave them zero here (the
    // folder-import flow skips the SD-card confirm dialog where they'd
    // be displayed, so the values are unused upstream).
    let label = source_pb
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "folder".to_string());
    let synthetic = ImportSource {
        path: source_path,
        label,
        read_only: false,
        file_count: 0,
        total_bytes: 0,
        detected_kind: None,
    };

    let cancel_flag = state.cancel_flag.clone();
    let running = state.running.clone();
    let unknown_receiver = state.unknown_receiver.clone();
    let sources = vec![synthetic];

    tauri::async_runtime::spawn_blocking(move || {
        // wipe_eligible=false → the source folder is never touched.
        let result = run_pipeline(
            &app,
            &cancel_flag,
            &unknown_receiver,
            &root_path,
            &sources,
            false,
        );
        let _ = app.emit("import:complete", &result);
        running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

/// Apply the same `<root>/Videos` → `<root>` adjustment that
/// `run_pipeline` does, so callers reasoning about the library root
/// before kicking off the pipeline see the same path the pipeline
/// will actually use.
fn resolve_library_root(root_path: &str) -> PathBuf {
    let given = PathBuf::from(root_path);
    if given
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("Videos"))
        .unwrap_or(false)
    {
        given.parent().unwrap_or(&given).to_path_buf()
    } else {
        given
    }
}

/// Refuse a folder import that would re-import the library into
/// itself: source IS the library root, source is INSIDE the library
/// root, or source CONTAINS the library root. Any of those three
/// would either re-stage already-imported files or stage files into
/// their own destination tree — both produce nonsense.
///
/// Canonicalizes both paths first so `.`, `..`, and symlinks are
/// compared consistently. Falls back to lexical comparison when
/// canonicalization fails (e.g. the path doesn't exist yet).
fn guard_against_self_import(source: &Path, library_root: &Path) -> Result<(), AppError> {
    let src = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let root = library_root
        .canonicalize()
        .unwrap_or_else(|_| library_root.to_path_buf());

    if src == root {
        return Err(AppError::Internal(
            "Source folder is the library root — pick a folder outside the library.".into(),
        ));
    }
    if src.starts_with(&root) {
        return Err(AppError::Internal(format!(
            "Source folder is inside the library at {} — pick a folder outside the library.",
            root.display()
        )));
    }
    if root.starts_with(&src) {
        return Err(AppError::Internal(format!(
            "Source folder contains the library at {} — pick a more specific folder.",
            root.display()
        )));
    }
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

/// Run the full import pipeline against one or more sources.
///
/// `wipe_eligible` controls whether the wipe phase may run on a source
/// after staging completes. SD-card import passes `true` (the wipe is
/// the whole point — free the card after verified copy). Folder import
/// passes `false` (the user keeps the original folder; we never touch
/// it). The wipe is *additionally* gated on `all_verified`, the cancel
/// flag, and `source.read_only`, regardless of `wipe_eligible`.
fn run_pipeline(
    app: &tauri::AppHandle,
    cancel_flag: &AtomicBool,
    unknown_receiver: &Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
    root_path: &str,
    sources: &[ImportSource],
    wipe_eligible: bool,
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
            wipe_eligible,
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

// Eight arguments is one over clippy's default threshold. Refactoring
// into a context struct would be churn for the call sites without
// improving readability — this function is private and only has two
// callers (the SD-card and folder-import top-level commands).
#[allow(clippy::too_many_arguments)]
fn process_source(
    source: &ImportSource,
    root_path: &Path,
    config: &mut ImportConfig,
    cancel_flag: &AtomicBool,
    unknown_receiver: &Arc<Mutex<Option<mpsc::Receiver<Vec<UnknownFileDecision>>>>>,
    app: &tauri::AppHandle,
    logger: &mut ImportLogger,
    wipe_eligible: bool,
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

    // Phase 3: Wipe (only if eligible, all verified, not cancelled, not read-only)
    let all_verified = manifest.iter().all(|e| e.verified);
    if wipe_eligible
        && all_verified
        && !cancel_flag.load(Ordering::Relaxed)
        && !source.read_only
    {
        match wipe::wipe_source(source, cancel_flag, app, logger) {
            Ok(()) => result.source_wiped = true,
            Err(e) => {
                logger.warn(&format!("Wipe failed: {e}"));
                result.warnings.push(format!("Wipe failed: {e}"));
            }
        }
    } else if wipe_eligible && source.read_only {
        logger.info("Skipping wipe: source is read-only");
    } else if !wipe_eligible {
        // Folder import: source is left alone by design. No log line —
        // a successful folder import shouldn't pretend a wipe was
        // considered.
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

#[cfg(test)]
mod folder_import_tests {
    use super::*;
    use std::fs;

    fn unique_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        std::env::temp_dir().join(format!(
            "tripviewer-folderimport-{tag}-{}-{}",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn guard_rejects_source_equal_to_library_root() {
        let dir = unique_dir("eq");
        fs::create_dir_all(&dir).unwrap();
        let result = guard_against_self_import(&dir, &dir);
        assert!(result.is_err(), "source == root must be refused");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn guard_rejects_source_inside_library_root() {
        let root = unique_dir("inside-root");
        let inner = root.join("subdir");
        fs::create_dir_all(&inner).unwrap();
        let result = guard_against_self_import(&inner, &root);
        assert!(result.is_err(), "source nested under root must be refused");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_rejects_source_containing_library_root() {
        let outer = unique_dir("contains");
        let root = outer.join("library");
        fs::create_dir_all(&root).unwrap();
        let result = guard_against_self_import(&outer, &root);
        assert!(
            result.is_err(),
            "source containing the library must be refused"
        );
        let _ = fs::remove_dir_all(&outer);
    }

    #[test]
    fn guard_accepts_disjoint_paths() {
        let a = unique_dir("disjoint-a");
        let b = unique_dir("disjoint-b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let result = guard_against_self_import(&a, &b);
        assert!(result.is_ok(), "unrelated source and root must pass: {result:?}");
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn resolve_library_root_strips_videos_suffix() {
        let parent = unique_dir("rootres");
        let videos = parent.join("Videos");
        fs::create_dir_all(&videos).unwrap();

        let resolved = resolve_library_root(&videos.to_string_lossy());
        // Parent is the resolved root.
        assert_eq!(resolved, parent);

        // A path that doesn't end in /Videos is left alone.
        let plain = resolve_library_root(&parent.to_string_lossy());
        assert_eq!(plain, parent);

        let _ = fs::remove_dir_all(&parent);
    }
}
