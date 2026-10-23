//! Tauri IPC commands for clip export. Same shape as the timelapse ones:
//! `start_export` spawns a blocking worker and returns immediately;
//! progress and the final result arrive as events; `cancel_export` flips
//! the shared cancel flag.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::app_settings::AppSettingsHandle;
use crate::error::AppError;
use crate::export::pipeline::run_export;
use crate::export::plan::StartExportArgs;
use crate::timelapse::CancelFlag;

/// `Some(flag)` while an export is running.
#[derive(Default)]
pub struct ExportState {
    cancel: Option<CancelFlag>,
}

pub type SharedExportState = Arc<Mutex<ExportState>>;

pub fn new_shared_state() -> SharedExportState {
    Arc::new(Mutex::new(ExportState::default()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgress {
    fraction: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportDone {
    ok: bool,
    cancelled: bool,
    output_path: Option<String>,
    warnings: Vec<String>,
    error: Option<String>,
}

/// Start exporting a composite clip. Errors up front if ffmpeg is not
/// configured (the Timelapse tab owns that setting) or another export is
/// already running; everything after that is reported through
/// `export:progress` and `export:done` events.
#[tauri::command]
pub async fn start_export(
    args: StartExportArgs,
    app: AppHandle,
    settings: State<'_, AppSettingsHandle>,
    state: State<'_, SharedExportState>,
) -> Result<(), AppError> {
    let ffmpeg_path = settings.read().ffmpeg_path.ok_or_else(|| {
        AppError::Internal("ffmpeg not configured — set its path in the Timelapse tab first".into())
    })?;

    let cancel: CancelFlag = Arc::new(AtomicBool::new(false));
    {
        let mut st = state
            .lock()
            .map_err(|_| AppError::Internal("export state poisoned".into()))?;
        if st.cancel.is_some() {
            return Err(AppError::Internal("an export is already running".into()));
        }
        st.cancel = Some(cancel.clone());
    }

    let state_clone: SharedExportState = (*state).clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app.clone();
        let on_progress = Arc::new(move |fraction: f64| {
            let _ = progress_app.emit("export:progress", ExportProgress { fraction });
        });
        let result = run_export(&args, &ffmpeg_path, &cancel, on_progress);
        if let Ok(mut st) = state_clone.lock() {
            st.cancel = None;
        }
        let done = match result {
            Ok(outcome) => ExportDone {
                ok: true,
                cancelled: false,
                output_path: Some(outcome.output_path.to_string_lossy().into_owned()),
                warnings: outcome.warnings,
                error: None,
            },
            Err(AppError::Internal(m)) if m == "cancelled" => ExportDone {
                ok: false,
                cancelled: true,
                output_path: None,
                warnings: vec![],
                error: None,
            },
            Err(e) => ExportDone {
                ok: false,
                cancelled: false,
                output_path: None,
                warnings: vec![],
                error: Some(e.to_string()),
            },
        };
        let _ = app.emit("export:done", done);
    });

    Ok(())
}

#[tauri::command]
pub async fn cancel_export(state: State<'_, SharedExportState>) -> Result<(), AppError> {
    let st = state
        .lock()
        .map_err(|_| AppError::Internal("export state poisoned".into()))?;
    if let Some(flag) = st.cancel.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}
