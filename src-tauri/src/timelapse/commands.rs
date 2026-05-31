//! Tauri IPC commands for the timelapse pipeline. Structure mirrors
//! `scans::commands`: `start_*` spawns a blocking worker and returns
//! immediately; `cancel_*` flips the shared cancel flag.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::app_settings::AppSettingsHandle;
use crate::archive::{require_db, ArchiveSlot};
use crate::db;
use crate::error::AppError;
use crate::timelapse::concurrency::{detect_recommended_concurrency, MAX_CONCURRENCY};
use crate::timelapse::ffmpeg::{self, Encoder};
use crate::timelapse::types::{Channel, FfmpegCapabilities, JobScope, Tier};
use crate::timelapse::worker::{new_cancel_flag, run_timelapse_loop, SharedWorkerState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseSettings {
    pub ffmpeg_path: Option<String>,
    pub capabilities: Option<FfmpegCapabilities>,
}

#[tauri::command]
pub async fn get_timelapse_settings(
    settings: State<'_, AppSettingsHandle>,
) -> Result<TimelapseSettings, AppError> {
    let s = settings.read();
    let capabilities = match (s.ffmpeg_version, s.nvenc_hevc) {
        (Some(v), Some(n)) => Some(FfmpegCapabilities {
            version: v,
            nvenc_hevc: n,
        }),
        _ => None,
    };
    Ok(TimelapseSettings {
        ffmpeg_path: s.ffmpeg_path,
        capabilities,
    })
}

/// Erase the cached ffmpeg path and capability flags. Used by the
/// FfmpegConfig modal's Clear button — lets the user disable timelapse
/// encoding (e.g. switching to a machine without ffmpeg) and exposes the
/// "not configured" UI path for testing.
#[tauri::command]
pub async fn clear_timelapse_settings(
    settings: State<'_, AppSettingsHandle>,
) -> Result<(), AppError> {
    settings.update(|s| {
        s.ffmpeg_path = None;
        s.ffmpeg_version = None;
        s.nvenc_hevc = None;
    })
}

/// macOS only: returns true if the file at `path` carries the
/// `com.apple.quarantine` extended attribute. Frontend calls this
/// after `test_ffmpeg` fails to decide whether to offer the
/// "clear quarantine" recovery path. Returns false on every other
/// platform so the frontend can call it unconditionally.
#[tauri::command]
pub async fn is_ffmpeg_quarantined(path: String) -> Result<bool, AppError> {
    #[cfg(target_os = "macos")]
    {
        Ok(ffmpeg::has_quarantine_attr(&path))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(false)
    }
}

/// macOS only: strips `com.apple.quarantine` from the file at `path`
/// so Gatekeeper will let it run. Equivalent to right-clicking the
/// binary in Finder and choosing Open. The user has to click a button
/// to invoke this; the app never strips xattrs silently.
#[tauri::command]
pub async fn clear_ffmpeg_quarantine(path: String) -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        let metadata = std::fs::metadata(&path)
            .map_err(|e| AppError::Internal(format!("cannot stat {path}: {e}")))?;
        if !metadata.is_file() {
            return Err(AppError::Internal(format!(
                "{path} is not a regular file"
            )));
        }
        ffmpeg::clear_quarantine_attr(&path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err(AppError::Internal(
            "clear_ffmpeg_quarantine is only available on macOS".into(),
        ))
    }
}

/// Run `ffmpeg -version` and `-encoders` on the given path, cache the
/// result to per-machine settings, and return it. The frontend's
/// "Test" button calls this.
#[tauri::command]
pub async fn test_ffmpeg(
    path: String,
    settings: State<'_, AppSettingsHandle>,
) -> Result<FfmpegCapabilities, AppError> {
    let caps = ffmpeg::probe_ffmpeg(&path)?;
    let caps_for_save = caps.clone();
    settings.update(move |s| {
        s.ffmpeg_path = Some(path);
        s.ffmpeg_version = Some(caps_for_save.version);
        s.nvenc_hevc = Some(caps_for_save.nvenc_hevc);
    })?;
    Ok(caps)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTimelapseArgs {
    pub trip_ids: Option<Vec<String>>,
    pub tiers: Vec<Tier>,
    pub channels: Vec<Channel>,
    pub scope: JobScope,
}

/// Kick off a background timelapse run. Returns immediately; progress
/// arrives via `timelapse:start` / `timelapse:progress` / `timelapse:done`
/// events. Errors if ffmpeg is not yet configured or another run is
/// already active.
#[tauri::command]
pub async fn start_timelapse(
    args: StartTimelapseArgs,
    app: AppHandle,
    slot: State<'_, ArchiveSlot>,
    settings: State<'_, AppSettingsHandle>,
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
    let db = require_db(&slot)?;
    let s = settings.read();
    let ffmpeg_path = s.ffmpeg_path.ok_or_else(|| {
        AppError::Internal("ffmpeg not configured — set path in settings first".into())
    })?;
    let caps = match (s.ffmpeg_version, s.nvenc_hevc) {
        (Some(v), Some(n)) => FfmpegCapabilities {
            version: v,
            nvenc_hevc: n,
        },
        _ => {
            return Err(AppError::Internal(
                "ffmpeg capabilities not cached — run the Test button first".into(),
            ))
        }
    };
    let concurrency_override = s.timelapse_max_concurrent_jobs.map(|n| n as usize);

    // Concurrency: explicit override wins, otherwise auto-detect from
    // hardware. Both paths get clamped to `1..=MAX_CONCURRENCY` so a
    // garbage setting can't crash the worker pool or exhaust GPU
    // sessions.
    let encoder = Encoder::pick(&caps);
    let concurrency = concurrency_override
        .unwrap_or_else(|| detect_recommended_concurrency(encoder))
        .clamp(1, MAX_CONCURRENCY);
    eprintln!(
        "[timelapse] starting: encoder={} concurrency={}",
        encoder.as_str(),
        concurrency
    );

    let cancel = {
        let mut state = worker_state
            .lock()
            .map_err(|_| AppError::Internal("timelapse worker state poisoned".into()))?;
        if state.running {
            return Err(AppError::Internal("timelapse already running".into()));
        }
        let flag = new_cancel_flag();
        state.running = true;
        state.cancel = Some(flag.clone());
        flag
    };

    let app_clone = app.clone();
    let db_clone = db.clone();
    let state_clone: SharedWorkerState = (*worker_state).clone();
    let archive_root = db.archive_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        // Refresh the trip/segment tables from disk before encoding so
        // newly imported (or hand-copied) files show up in the work
        // list. Without this, users had to remember to click Scan
        // before Timelapse; new trips were silently skipped because
        // build_work_list reads from the `trips` table.
        //
        // Failure here is logged but not fatal: a stale library
        // (e.g. archive drive unplugged) still has whatever rows the
        // last successful scan left behind, and the encode can run on
        // those rather than refusing to do anything.
        let _ = app_clone.emit("timelapse:scanning", true);
        let scan_started_ms = chrono::Utc::now().timestamp_millis();
        match crate::scan::scan_folder_sync(&archive_root, &archive_root) {
            Ok(result) => {
                if let Ok(mut conn) = db_clone.lock() {
                    if let Err(e) = crate::db::segments::persist_and_gc(
                        &mut conn,
                        &result.trips,
                        scan_started_ms,
                        &archive_root,
                    ) {
                        eprintln!("[timelapse] pre-scan persist_and_gc failed: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("[timelapse] pre-scan failed (continuing with existing DB rows): {e}");
            }
        }
        let _ = app_clone.emit("timelapse:scanning", false);

        run_timelapse_loop(
            app_clone,
            db_clone,
            state_clone,
            cancel,
            args.trip_ids,
            args.tiers,
            args.channels,
            args.scope,
            ffmpeg_path,
            caps,
            concurrency,
        );
    });

    Ok(())
}

#[tauri::command]
pub async fn cancel_timelapse(
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
    let state = worker_state
        .lock()
        .map_err(|_| AppError::Internal("timelapse worker state poisoned".into()))?;
    if let Some(flag) = state.cancel.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Move every orphan timelapse file under `<archive>/Timelapses/` to
/// trash. An orphan is a file named `{trip_id}_{tier}_{channel}.mp4`
/// where `trip_id` matches no row in `timelapse_jobs` — i.e. nothing
/// in the app references it. Files go to the OS trash so the user can
/// recover them if needed.
#[tauri::command]
pub async fn prune_orphan_timelapse_files(
    slot: State<'_, ArchiveSlot>,
) -> Result<crate::timelapse::cleanup::PruneSummary, AppError> {
    let db = require_db(&slot)?;
    crate::timelapse::cleanup::prune_orphan_timelapse_files(&db)
}

/// Read-only orphan count. Used by the frontend to decide whether to
/// surface the Prune button with a "needs attention" badge — without
/// this hint, users had no signal that orphans were sitting on disk.
#[tauri::command]
pub async fn count_orphan_timelapse_files(
    slot: State<'_, ArchiveSlot>,
) -> Result<u64, AppError> {
    let db = require_db(&slot)?;
    crate::timelapse::cleanup::count_orphan_timelapse_files(&db)
}

#[tauri::command]
pub async fn list_timelapse_jobs(
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<db::timelapse_jobs::TimelapseJobRow>, AppError> {
    let db = require_db(&slot)?;
    let archive_root = db.archive_root().to_path_buf();
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let mut rows = db::timelapse_jobs::list_all(&conn)?;
    // output_path is stored archive-relative (forward slashes). Rejoin
    // with the current archive root before handing rows to the frontend
    // so the video server gets an absolute filesystem path it can open.
    // Path::join naturally passes any legacy absolute value through
    // unchanged, which keeps un-migrated DBs functional in dev builds.
    for row in rows.iter_mut() {
        if let Some(rel) = row.output_path.as_deref() {
            row.output_path = Some(
                crate::paths::from_archive_relative(rel, &archive_root)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(rows)
}

/// On-disk archive completeness for a single trip. Used by the
/// Delete-Originals / Delete-Selected-Segments confirmations to BLOCK
/// the irreversible delete unless a usable timelapse genuinely exists on
/// disk — not just a `done` row in the DB (the file could have been
/// deleted out from under us, or live on an unplugged drive).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOnDisk {
    /// Count of `done` timelapse_jobs rows for the trip.
    pub done_jobs: usize,
    /// Of those, how many output files are actually present on disk.
    pub files_present: usize,
    /// Archive-relative paths of `done` jobs whose file is missing.
    pub missing_files: Vec<String>,
    /// True when the archive root (Timelapses dir) is reachable at all.
    /// False ⇒ the drive is likely unplugged; callers should treat the
    /// result as "unknown" rather than "files missing".
    pub archive_reachable: bool,
}

/// Verify a trip's timelapse outputs exist on disk right now. Live check
/// (no cached status) so it catches a file deleted since the last
/// startup sweep. `archive_reachable=false` distinguishes an unplugged
/// drive from genuinely-missing files so the UI never falsely blocks.
#[tauri::command]
pub async fn trip_archive_on_disk(
    trip_id: String,
    slot: State<'_, ArchiveSlot>,
) -> Result<ArchiveOnDisk, AppError> {
    let db = require_db(&slot)?;
    let archive_root = db.archive_root().to_path_buf();
    let archive_reachable = archive_root.join("Timelapses").exists();

    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let mut stmt = conn.prepare(
        "SELECT output_path FROM timelapse_jobs
         WHERE trip_id = ?1 AND status = ?2 AND output_path IS NOT NULL",
    )?;
    let rels: Vec<String> = stmt
        .query_map(
            rusqlite::params![trip_id, db::timelapse_jobs::STATUS_DONE],
            |r| r.get::<_, String>(0),
        )?
        .filter_map(Result::ok)
        .collect();

    let mut files_present = 0usize;
    let mut missing_files = Vec::new();
    for rel in &rels {
        let abs = crate::paths::from_archive_relative(rel, &archive_root);
        if abs.exists() {
            files_present += 1;
        } else {
            missing_files.push(rel.clone());
        }
    }
    Ok(ArchiveOnDisk {
        done_jobs: rels.len(),
        files_present,
        // If the drive is unplugged everything "looks" missing; don't
        // report that as a real gap.
        missing_files: if archive_reachable {
            missing_files
        } else {
            Vec::new()
        },
        archive_reachable,
    })
}
