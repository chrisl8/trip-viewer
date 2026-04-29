//! Tauri commands that drive the scan pipeline. `start_scan` spawns a
//! background worker on a blocking thread and returns immediately; the
//! worker emits `scan:start`/`scan:progress`/`scan:done` events.
//! `cancel_scan` sets the shared cancel flag so the worker stops at the
//! next safe point (between segments or inside scan-internal loops).

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::db::DbHandle;
use crate::error::AppError;
use crate::scans::coverage::TripScanCoverage;
use crate::scans::worker::{
    new_cancel_flag, run_scan_loop, ScanScope, SharedWorkerState,
};
use crate::scans::{coverage, registry, CostTier};

/// Describe every registered scan so the ScanView can render checkboxes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub version: u32,
    pub cost_tier: CostTier,
    pub emits: Vec<String>,
}

#[tauri::command]
pub async fn list_scans() -> Vec<ScanDescriptor> {
    registry()
        .iter()
        .map(|s| ScanDescriptor {
            id: s.id().to_string(),
            display_name: s.display_name().to_string(),
            description: s.description().to_string(),
            version: s.version(),
            cost_tier: s.cost_tier(),
            emits: s.emits().iter().map(|s| (*s).to_string()).collect(),
        })
        .collect()
}

/// Kick off a background scan. Returns immediately; progress arrives via
/// events. Errors if a scan is already running — the caller should
/// cancel first. `trip_ids` is an optional whitelist; when present the
/// worker only processes segments belonging to those trips (used by the
/// per-trip Rebuild button on the Trips table). `None` = whole library.
#[tauri::command]
pub async fn start_scan(
    scan_ids: Vec<String>,
    scope: ScanScope,
    trip_ids: Option<Vec<String>>,
    app: AppHandle,
    db: State<'_, DbHandle>,
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
    let cancel = {
        let mut state = worker_state
            .lock()
            .map_err(|_| AppError::Internal("scan worker state poisoned".into()))?;
        if state.running {
            return Err(AppError::Internal("scan already running".into()));
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
        run_scan_loop(
            app_clone,
            db_clone,
            state_clone,
            cancel,
            scan_ids,
            scope,
            trip_ids,
        );
    });

    Ok(())
}

/// Compute per-trip × per-scan coverage for the Trips table on the
/// Scan view. Returns a row per trip, with one `ScanCoverage` per
/// registered scan (whether or not the user has it selected). Cheap
/// — two GROUP BY queries plus a HashMap merge.
#[tauri::command]
pub async fn list_scan_coverage(
    db: State<'_, DbHandle>,
) -> Result<Vec<TripScanCoverage>, AppError> {
    coverage::list_scan_coverage(&db)
}

/// Request that the running scan stop at the next safe point. No-op if
/// nothing is running.
#[tauri::command]
pub async fn cancel_scan(
    worker_state: State<'_, SharedWorkerState>,
) -> Result<(), AppError> {
    let state = worker_state
        .lock()
        .map_err(|_| AppError::Internal("scan worker state poisoned".into()))?;
    if let Some(flag) = state.cancel.as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}
