//! Background startup pipeline.
//!
//! Heavy startup work — GPS backfill for pre-feature timelapses,
//! stale-job recovery, one-time cross-OS path rewrites — used to run
//! synchronously inside Tauri's `setup()` closure, before
//! `window.show()`. On a heavy library that meant the user stared at
//! nothing for many seconds. We now show the window quickly and run
//! these tasks on a background blocking thread, reporting progress
//! through a managed snapshot + `startup:*` events. The frontend
//! renders a full-viewport splash until `done = true`.
//!
//! Wired up from `lib.rs::run` — see the `setup()` closure for how
//! `StartupState` is managed and how `run()` is spawned.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_settings::AppSettingsHandle;
use crate::db::DbHandle;
use crate::migration_v2;
use crate::timelapse::cleanup;

const TASK_CLEANUP: &str = "cleanup_stale_jobs";
const TASK_GPS_BACKFILL: &str = "gps_backfill";
const TASK_CROSS_OS_REBUILD: &str = "cross_os_rebuild";

/// Match the per-launch cap that `backfill_trip_gps` enforced when it
/// ran inline in `setup()`. A library with thousands of pre-feature
/// trips converges across a handful of launches rather than tail-blocking
/// any single startup.
const GPS_BACKFILL_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupTask {
    pub id: String,
    pub label: String,
    pub description: String,
    pub current: usize,
    /// `None` for indeterminate work (frontend renders a spinner).
    pub total: Option<usize>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupSnapshot {
    pub tasks: Vec<StartupTask>,
    pub done: bool,
}

pub type StartupState = Arc<Mutex<StartupSnapshot>>;

pub fn new_state() -> StartupState {
    // Default is "nothing to do" — `run()` overwrites this when work
    // is queued. A frontend that queries before `run()` fires sees a
    // done snapshot and never flashes the splash.
    Arc::new(Mutex::new(StartupSnapshot {
        tasks: Vec::new(),
        done: true,
    }))
}

pub fn mark_no_work(state: &StartupState, app: &AppHandle) {
    if let Ok(mut s) = state.lock() {
        s.tasks.clear();
        s.done = true;
        let _ = app.emit("startup:done", s.clone());
    }
}

fn plan_tasks(db: &DbHandle, settings: &AppSettingsHandle) -> Vec<StartupTask> {
    let mut tasks = Vec::new();

    let stale = cleanup::list_stale_jobs(db).map(|v| v.len()).unwrap_or(0);
    if stale > 0 {
        tasks.push(StartupTask {
            id: TASK_CLEANUP.to_string(),
            label: "Recovering interrupted renders".to_string(),
            description:
                "Clearing partial timelapse files left over from a previous run."
                    .to_string(),
            current: 0,
            total: Some(stale),
            status: TaskStatus::Pending,
        });
    }

    let gps_count = cleanup::backfill_candidates(db, GPS_BACKFILL_LIMIT)
        .map(|v| v.len())
        .unwrap_or(0);
    if gps_count > 0 {
        tasks.push(StartupTask {
            id: TASK_GPS_BACKFILL.to_string(),
            label: "Backing up GPS data".to_string(),
            description:
                "Saving GPS tracks for trips that were timelapsed before this feature \
                 shipped, so map and speed data stay available if originals are deleted."
                    .to_string(),
            current: 0,
            total: Some(gps_count),
            status: TaskStatus::Pending,
        });
    }

    let archive_root = db.archive_root().to_string_lossy().into_owned();
    let needs_cross_os = !settings
        .read()
        .cross_os_migrated_archives
        .iter()
        .any(|p| p == &archive_root);
    if needs_cross_os {
        tasks.push(StartupTask {
            id: TASK_CROSS_OS_REBUILD.to_string(),
            label: "Adapting archive to this OS".to_string(),
            description:
                "Rewriting stored paths so segments resolve correctly on this \
                 platform. Runs once per archive."
                    .to_string(),
            current: 0,
            total: None,
            status: TaskStatus::Pending,
        });
    }

    tasks
}

fn emit_progress(state: &StartupState, app: &AppHandle) {
    if let Ok(s) = state.lock() {
        let _ = app.emit("startup:task-progress", s.clone());
    }
}

fn emit_done(state: &StartupState, app: &AppHandle) {
    if let Ok(s) = state.lock() {
        let _ = app.emit("startup:done", s.clone());
    }
}

fn with_task<F>(state: &StartupState, task_id: &str, f: F)
where
    F: FnOnce(&mut StartupTask),
{
    if let Ok(mut s) = state.lock() {
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == task_id) {
            f(t);
        }
    }
}

/// Background-task entrypoint. Runs the planned tasks sequentially,
/// mutating the shared `StartupState` and emitting an event after each
/// step. On entry the snapshot is overwritten with the planned tasks;
/// on exit `done = true` and a `startup:done` event is emitted.
///
/// Safe to call with no archive open / no pending work — it returns
/// quickly after marking the state done.
pub fn run(app: AppHandle, db: DbHandle) {
    let state = match app.try_state::<StartupState>() {
        Some(s) => (*s).clone(),
        None => {
            eprintln!("[startup] StartupState not managed — aborting");
            return;
        }
    };
    let settings_state = match app.try_state::<AppSettingsHandle>() {
        Some(s) => s,
        None => {
            eprintln!("[startup] AppSettingsHandle not managed — aborting");
            return;
        }
    };
    let settings: &AppSettingsHandle = &settings_state;

    let tasks = plan_tasks(&db, settings);

    // Unconditional fast pass: wipe any leftover scratch dirs under
    // <archive>/Timelapses/.tmp/. A previous session's hard exit or
    // a cancelled encode that couldn't sweep its own dir leaves
    // gigabytes of partial __multi_window_*.mp4 files behind; this
    // is safe to do at startup because no encode is running yet.
    cleanup::wipe_scratch_tree(&db);

    // Unconditional fast pass: stat every done row's output file and
    // flip missing ones to 'failed'. Cheap (one stat per row), so we
    // skip the splash-UI task plumbing — runs even when no other
    // task is planned. Catches "lying done" rows left behind by the
    // 0013 path migration when some output files didn't survive a
    // drive remount.
    if let Err(e) = cleanup::flag_missing_outputs(&db) {
        eprintln!("[startup] flag_missing_outputs failed: {e}");
    }

    // Second unconditional pass: try to recover any rows the previous
    // step just flagged by renaming on-disk files that still use the
    // pre-rewrite trip_id naming. This is the inverse of what
    // rebuild_for_cross_os does to DB rows — without it, the user
    // would have to re-encode files that already exist under a stale
    // name.
    if let Err(e) = cleanup::recover_orphan_outputs(&db, settings) {
        eprintln!("[startup] recover_orphan_outputs failed: {e}");
    }

    // Third unconditional pass: relink failed rows with NULL output_path
    // back to their on-disk file when one exists at the canonical
    // location. Recovers from the "Rebuild all over an archive-only
    // trip" bug that nulled output_path before the encode discovered
    // there were no source segments.
    if let Err(e) = cleanup::relink_present_outputs(&db) {
        eprintln!("[startup] relink_present_outputs failed: {e}");
    }

    if tasks.is_empty() {
        mark_no_work(&state, &app);
        return;
    }

    if let Ok(mut s) = state.lock() {
        s.tasks = tasks.clone();
        s.done = false;
    }
    emit_progress(&state, &app);

    for task in &tasks {
        match task.id.as_str() {
            TASK_CLEANUP => run_cleanup(&app, &state, &db),
            TASK_GPS_BACKFILL => run_gps_backfill(&app, &state, &db),
            TASK_CROSS_OS_REBUILD => run_cross_os(&app, &state, &db, settings),
            _ => {}
        }
    }

    if let Ok(mut s) = state.lock() {
        s.done = true;
    }
    emit_done(&state, &app);
}

fn run_cleanup(app: &AppHandle, state: &StartupState, db: &DbHandle) {
    with_task(state, TASK_CLEANUP, |t| t.status = TaskStatus::Running);
    emit_progress(state, app);

    let stale = match cleanup::list_stale_jobs(db) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[startup] list_stale_jobs failed: {e}");
            with_task(state, TASK_CLEANUP, |t| t.status = TaskStatus::Failed);
            emit_progress(state, app);
            return;
        }
    };
    // Recount in case the candidate set changed between plan and run.
    with_task(state, TASK_CLEANUP, |t| t.total = Some(stale.len()));

    for (i, row) in stale.iter().enumerate() {
        if let Err(e) = cleanup::reset_one_stale_job(db, row) {
            eprintln!("[startup] reset_one_stale_job failed: {e}");
        }
        with_task(state, TASK_CLEANUP, |t| t.current = i + 1);
        emit_progress(state, app);
    }

    if !stale.is_empty() {
        eprintln!(
            "[timelapse] cleanup: reset {} stale running job(s) to pending",
            stale.len()
        );
    }

    with_task(state, TASK_CLEANUP, |t| t.status = TaskStatus::Done);
    emit_progress(state, app);
}

fn run_gps_backfill(app: &AppHandle, state: &StartupState, db: &DbHandle) {
    with_task(state, TASK_GPS_BACKFILL, |t| {
        t.status = TaskStatus::Running;
    });
    emit_progress(state, app);

    let candidates = match cleanup::backfill_candidates(db, GPS_BACKFILL_LIMIT) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[startup] backfill_candidates failed: {e}");
            with_task(state, TASK_GPS_BACKFILL, |t| t.status = TaskStatus::Failed);
            emit_progress(state, app);
            return;
        }
    };
    with_task(state, TASK_GPS_BACKFILL, |t| {
        t.total = Some(candidates.len());
    });

    let mut written = 0usize;
    for (i, trip_id) in candidates.iter().enumerate() {
        match cleanup::backfill_one_trip(db, trip_id) {
            Ok(true) => written += 1,
            Ok(false) => {}
            Err(e) => {
                eprintln!("[startup] backfill_one_trip({trip_id}) failed: {e}");
            }
        }
        with_task(state, TASK_GPS_BACKFILL, |t| t.current = i + 1);
        emit_progress(state, app);
    }

    if written > 0 {
        eprintln!("[timelapse] gps backfill: persisted GPS for {written} trip(s)");
    }

    with_task(state, TASK_GPS_BACKFILL, |t| t.status = TaskStatus::Done);
    emit_progress(state, app);
}

fn run_cross_os(
    app: &AppHandle,
    state: &StartupState,
    db: &DbHandle,
    settings: &AppSettingsHandle,
) {
    with_task(state, TASK_CROSS_OS_REBUILD, |t| {
        t.status = TaskStatus::Running;
    });
    emit_progress(state, app);

    match migration_v2::rebuild_for_cross_os(db, settings) {
        Ok(_) => with_task(state, TASK_CROSS_OS_REBUILD, |t| {
            t.status = TaskStatus::Done;
        }),
        Err(e) => {
            eprintln!("[migration_v2] cross-OS rewrite failed: {e}");
            with_task(state, TASK_CROSS_OS_REBUILD, |t| {
                t.status = TaskStatus::Failed;
            });
        }
    }
    emit_progress(state, app);
}

#[tauri::command]
pub fn get_startup_status(state: tauri::State<'_, StartupState>) -> StartupSnapshot {
    match state.inner().lock() {
        Ok(s) => s.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}
