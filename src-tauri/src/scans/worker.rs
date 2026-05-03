//! Background scan supervisor. Walks a work list in a blocking thread,
//! runs each (segment, scan) pair, emits progress events, and honors a
//! shared cancellation flag. One scan at a time (no parallelism in v1 —
//! adding per-cost-tier semaphores is straightforward once heavy scans
//! demand it).
//!
//! Progress events are batched to ~4 Hz so the IPC channel doesn't
//! saturate on libraries with thousands of segments.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::scans::{find_scan, CancelFlag, ScanContext};

pub fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

#[derive(Default)]
pub struct ScanWorkerState {
    pub running: bool,
    pub cancel: Option<CancelFlag>,
}

pub type SharedWorkerState = Arc<Mutex<ScanWorkerState>>;

pub fn new_shared_state() -> SharedWorkerState {
    Arc::new(Mutex::new(ScanWorkerState::default()))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanScope {
    NewOnly,
    RescanStale,
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStartEvent {
    pub total: u64,
    pub scan_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressEvent {
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub current_segment_id: Option<String>,
    pub current_scan_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDoneEvent {
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub tags_emitted: u64,
    pub cancelled: bool,
}

/// Run the full scan loop on a blocking thread. Drops the `running`
/// flag in the worker state before returning regardless of outcome.
/// `trip_ids` is an optional whitelist; `None` means "every trip in
/// the library" (the default Start-button behavior). When set, only
/// segments belonging to those trips are scanned (per-trip Rebuild).
pub fn run_scan_loop(
    app: AppHandle,
    db: DbHandle,
    state: SharedWorkerState,
    cancel: CancelFlag,
    scan_ids: Vec<String>,
    scope: ScanScope,
    trip_ids: Option<Vec<String>>,
) {
    let result = run_inner(&app, &db, &cancel, &scan_ids, scope, trip_ids.as_deref());
    // Always clear running=false so future start_scan calls aren't blocked.
    if let Ok(mut guard) = state.lock() {
        guard.running = false;
        guard.cancel = None;
    }
    if let Err(e) = result {
        eprintln!("[scans] worker loop errored: {e}");
    }
}

fn run_inner(
    app: &AppHandle,
    db: &DbHandle,
    cancel: &CancelFlag,
    scan_ids: &[String],
    scope: ScanScope,
    trip_ids: Option<&[String]>,
) -> Result<(), AppError> {
    let (segments, places) = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut segs = db::segments::all_segments(&conn)?;
        if let Some(ids) = trip_ids {
            let trip_set: std::collections::HashSet<&str> =
                ids.iter().map(|s| s.as_str()).collect();
            segs.retain(|s| trip_set.contains(s.trip_id.as_str()));
        }
        // Drop segments whose master file is missing on disk. They
        // aren't scannable units — `File::open` would error with
        // NotFound and the result would land as `status='error'` in
        // `scan_runs`, which makes the trip's coverage pill show red
        // for what's really just a deleted-on-disk file. Filtering
        // here keeps `total` accurate (so the progress bar reflects
        // real work) and prevents writing phantom-failure rows.
        // Tombstones (`is_tombstone = 1`) have an empty `master_path`
        // and no file by design — same exclusion, but keyed on the
        // flag so we don't even hit the filesystem for them.
        segs.retain(|s| !s.is_tombstone && std::path::Path::new(&s.master_path).exists());
        let pls = db::places::list_places(&conn).unwrap_or_default();
        (segs, pls)
    };

    // Plan all (segment, scan) pairs upfront so `total` is accurate.
    let mut work: Vec<(usize, String)> = Vec::new();
    for (idx, segment) in segments.iter().enumerate() {
        for scan_id in scan_ids {
            if should_run(db, &segment.id, scan_id, scope)? {
                work.push((idx, scan_id.clone()));
            }
        }
    }

    let total = work.len() as u64;
    let _ = app.emit(
        "scan:start",
        ScanStartEvent {
            total,
            scan_ids: scan_ids.to_vec(),
        },
    );

    let mut done: u64 = 0;
    let mut failed: u64 = 0;
    let mut tags_emitted: u64 = 0;
    let mut last_emit = Instant::now();
    const EMIT_INTERVAL: Duration = Duration::from_millis(250);

    for (seg_idx, scan_id) in work {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let segment = &segments[seg_idx];
        let Some(scan) = find_scan(&scan_id) else {
            continue;
        };

        // Top-of-loop progress emit. Same rationale as the timelapse
        // worker: putting it here (rather than after scan.run) means
        // the final iteration's tally always reaches the frontend via
        // the post-loop emit below, and the `current_*` fields
        // describe the upcoming item ("now scanning…") rather than
        // the just-finished one.
        if last_emit.elapsed() >= EMIT_INTERVAL {
            let _ = app.emit(
                "scan:progress",
                ScanProgressEvent {
                    total,
                    done,
                    failed,
                    current_segment_id: Some(segment.id.clone()),
                    current_scan_id: Some(scan.id().to_string()),
                },
            );
            last_emit = Instant::now();
        }

        let ctx = ScanContext {
            segment,
            cancel,
            places: &places,
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        match scan.run(&ctx) {
            Ok(tags) => {
                tags_emitted += tags.len() as u64;
                let mut conn = db
                    .lock()
                    .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
                db::tags::commit_scan_run(
                    &mut conn,
                    &segment.id,
                    scan.id(),
                    scan.version(),
                    "ok",
                    None,
                    &tags,
                    now_ms,
                )?;
                done += 1;
            }
            Err(e) => {
                failed += 1;
                if let Ok(mut conn) = db.lock() {
                    let _ = db::tags::commit_scan_run(
                        &mut conn,
                        &segment.id,
                        scan.id(),
                        scan.version(),
                        "error",
                        Some(&e.to_string()),
                        &[],
                        now_ms,
                    );
                }
            }
        }
    }

    // Final unthrottled progress emit so the bar always lands on the
    // closing tally (e.g. 4/4 instead of 3/4 if the last iteration
    // happened inside the throttle window). `scan:done` doesn't
    // update the progress payload on the frontend.
    let _ = app.emit(
        "scan:progress",
        ScanProgressEvent {
            total,
            done,
            failed,
            current_segment_id: None,
            current_scan_id: None,
        },
    );

    let cancelled = cancel.load(Ordering::Relaxed);
    let _ = app.emit(
        "scan:done",
        ScanDoneEvent {
            total,
            done,
            failed,
            tags_emitted,
            cancelled,
        },
    );

    Ok(())
}

fn should_run(
    db: &DbHandle,
    segment_id: &str,
    scan_id: &str,
    scope: ScanScope,
) -> Result<bool, AppError> {
    let Some(scan) = find_scan(scan_id) else {
        return Ok(false);
    };
    let current_version = scan.version();

    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT version, status FROM scan_runs WHERE segment_id = ?1 AND scan_id = ?2",
            rusqlite::params![segment_id, scan_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    Ok(match (scope, row) {
        (ScanScope::All, _) => true,
        (ScanScope::NewOnly, None) => true,
        (ScanScope::NewOnly, Some(_)) => false,
        (ScanScope::RescanStale, None) => true,
        (ScanScope::RescanStale, Some((v, _))) => (v as u32) < current_version,
    })
}
