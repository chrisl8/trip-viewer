//! Tauri IPC commands for the timelapse pipeline. Structure mirrors
//! `scans::commands`: `start_*` spawns a blocking worker and returns
//! immediately; `cancel_*` flips the shared cancel flag.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::app_settings::AppSettingsHandle;
use crate::db::{self, DbHandle};
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
    db: State<'_, DbHandle>,
    settings: State<'_, AppSettingsHandle>,
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
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
    let db_clone: DbHandle = (*db).clone();
    let state_clone: SharedWorkerState = (*worker_state).clone();
    tauri::async_runtime::spawn_blocking(move || {
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

#[tauri::command]
pub async fn list_timelapse_jobs(
    db: State<'_, DbHandle>,
) -> Result<Vec<db::timelapse_jobs::TimelapseJobRow>, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::list_all(&conn)
}
