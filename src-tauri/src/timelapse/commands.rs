//! Tauri IPC commands for the timelapse pipeline. Structure mirrors
//! `scans::commands`: `start_*` spawns a blocking worker and returns
//! immediately; `cancel_*` flips the shared cancel flag.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::timelapse::ffmpeg;
use crate::timelapse::types::{Channel, FfmpegCapabilities, JobScope, Tier};
use crate::timelapse::worker::{new_cancel_flag, run_timelapse_loop, SharedWorkerState};

const SETTING_FFMPEG_PATH: &str = "ffmpeg_path";
const SETTING_FFMPEG_VERSION: &str = "ffmpeg_version";
const SETTING_NVENC_HEVC: &str = "nvenc_hevc";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseSettings {
    pub ffmpeg_path: Option<String>,
    pub capabilities: Option<FfmpegCapabilities>,
}

#[tauri::command]
pub async fn get_timelapse_settings(
    db: State<'_, DbHandle>,
) -> Result<TimelapseSettings, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let ffmpeg_path = db::settings::get(&conn, SETTING_FFMPEG_PATH)?;
    let version = db::settings::get(&conn, SETTING_FFMPEG_VERSION)?;
    let nvenc = db::settings::get(&conn, SETTING_NVENC_HEVC)?;
    let capabilities = match (version, nvenc) {
        (Some(v), Some(n)) => Some(FfmpegCapabilities {
            version: v,
            nvenc_hevc: n == "1" || n == "true",
        }),
        _ => None,
    };
    Ok(TimelapseSettings {
        ffmpeg_path,
        capabilities,
    })
}

/// Run `ffmpeg -version` and `-encoders` on the given path, cache the
/// result to the `settings` table, and return it. The frontend's
/// "Test" button calls this.
#[tauri::command]
pub async fn test_ffmpeg(
    path: String,
    db: State<'_, DbHandle>,
) -> Result<FfmpegCapabilities, AppError> {
    let caps = ffmpeg::probe_ffmpeg(&path)?;
    {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        db::settings::set(&conn, SETTING_FFMPEG_PATH, &path)?;
        db::settings::set(&conn, SETTING_FFMPEG_VERSION, &caps.version)?;
        db::settings::set(
            &conn,
            SETTING_NVENC_HEVC,
            if caps.nvenc_hevc { "1" } else { "0" },
        )?;
    }
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
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
    let (ffmpeg_path, caps) = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let path = db::settings::get(&conn, SETTING_FFMPEG_PATH)?.ok_or_else(|| {
            AppError::Internal("ffmpeg not configured — set path in settings first".into())
        })?;
        let version = db::settings::get(&conn, SETTING_FFMPEG_VERSION)?;
        let nvenc = db::settings::get(&conn, SETTING_NVENC_HEVC)?;
        let caps = match (version, nvenc) {
            (Some(v), Some(n)) => FfmpegCapabilities {
                version: v,
                nvenc_hevc: n == "1" || n == "true",
            },
            _ => {
                return Err(AppError::Internal(
                    "ffmpeg capabilities not cached — run the Test button first".into(),
                ))
            }
        };
        (path, caps)
    };

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
