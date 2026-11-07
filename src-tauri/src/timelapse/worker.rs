//! Background supervisor for timelapse generation. Mirrors the scan
//! pipeline's worker loop: spawn_blocking thread, `Arc<AtomicBool>`
//! cancel flag, progress events batched at ~4 Hz.
//!
//! Walks the work list of (trip_id, tier, channel) triples that need
//! encoding, invokes ffmpeg once per triple, and updates the
//! `timelapse_jobs` row as each completes.
//!
//! Stage 1 only iterates over (trip × {8x} × {F}); Stage 2 widens to
//! all channels and Stage 3 adds the variable-speed 16x and 60x tiers.
//! The worker signature doesn't change between stages.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::gps;
use crate::model::GpsPoint;
use crate::scan::naming::CameraKind;
use crate::timelapse::events;
use crate::timelapse::ffmpeg::{self, EncodeArgs, Encoder};
use crate::timelapse::types::{Channel, EventWindow, FfmpegCapabilities, JobScope, Tier};
use crate::timelapse::CancelFlag;

pub fn new_cancel_flag() -> CancelFlag {
    Arc::new(AtomicBool::new(false))
}

#[derive(Default)]
pub struct TimelapseWorkerState {
    pub running: bool,
    pub cancel: Option<CancelFlag>,
}

pub type SharedWorkerState = Arc<Mutex<TimelapseWorkerState>>;

pub fn new_shared_state() -> SharedWorkerState {
    Arc::new(Mutex::new(TimelapseWorkerState::default()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseStartEvent {
    pub total: u64,
    pub tiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseProgressEvent {
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub current_trip_id: Option<String>,
    pub current_tier: Option<String>,
    pub current_channel: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseDoneEvent {
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub cancelled: bool,
}

/// One unit of work for the worker loop.
struct WorkItem {
    trip_id: String,
    tier: Tier,
    channel: Channel,
}

/// Run the timelapse encode loop on a blocking thread. Always clears
/// `running=false` before returning so future start calls aren't
/// blocked, mirroring `scans::worker::run_scan_loop`.
///
/// `concurrency` is the worker pool size used inside each per-trip
/// chunk: `1` reproduces the old strictly-sequential behavior, larger
/// values fan out the per-trip (tier × channel) jobs to multiple
/// concurrent ffmpeg processes.
#[allow(clippy::too_many_arguments)]
pub fn run_timelapse_loop(
    app: AppHandle,
    db: DbHandle,
    state: SharedWorkerState,
    cancel: CancelFlag,
    trip_ids: Option<Vec<String>>,
    tiers: Vec<Tier>,
    channels: Vec<Channel>,
    scope: JobScope,
    ffmpeg_path: String,
    caps: FfmpegCapabilities,
    concurrency: usize,
) {
    let result = run_inner(
        &app,
        &db,
        &cancel,
        trip_ids,
        &tiers,
        &channels,
        scope,
        &ffmpeg_path,
        &caps,
        concurrency,
    );
    if let Ok(mut guard) = state.lock() {
        guard.running = false;
        guard.cancel = None;
    }
    if let Err(e) = result {
        eprintln!("[timelapse] worker loop errored: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    app: &AppHandle,
    db: &DbHandle,
    cancel: &CancelFlag,
    trip_ids: Option<Vec<String>>,
    tiers: &[Tier],
    channels: &[Channel],
    scope: JobScope,
    ffmpeg_path: &str,
    caps: &FfmpegCapabilities,
    concurrency: usize,
) -> Result<(), AppError> {
    let encoder = Encoder::pick(caps);
    let output_root = resolve_output_root(db)?;

    let work = build_work_list(db, trip_ids.as_deref(), tiers, channels, scope)?;
    let total = work.len() as u64;

    let _ = app.emit(
        "timelapse:start",
        TimelapseStartEvent {
            total,
            tiers: tiers.iter().map(|t| t.as_str().to_string()).collect(),
        },
    );

    // libx265 needs its internal thread pool sized to a fair share of
    // CPU when N>1 ffmpegs run in parallel; otherwise each x265
    // invocation spawns all-cores and N of them thrash the OS
    // scheduler. NVENC's encode threads live on the GPU, so the cap
    // doesn't apply there.
    let cpu_pool_threads: Option<usize> = if matches!(encoder, Encoder::LibX265) && concurrency > 1
    {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Some((cores / concurrency).max(1))
    } else {
        None
    };

    // Cross-worker progress state. `last_emit` gates the 250 ms
    // throttle that keeps IPC traffic down when many workers report
    // simultaneously; counters are summed at the end for the closing
    // emit.
    let done = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let last_emit = Mutex::new(Instant::now());
    const EMIT_INTERVAL: Duration = Duration::from_millis(250);

    // Group the flat work list into per-trip slices. The list is
    // already trip-grouped (build_work_list's outer loop is trips),
    // so we walk it once and slice on trip_id boundaries.
    let chunks = group_work_by_trip(&work);
    let pool_size = concurrency.max(1);

    for chunk in &chunks {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let trip_id = chunk[0].trip_id.as_str();

        // Build per-trip context once before the parallel block. This
        // is the same GPS stitch + event detection the sequential
        // loop performed once per trip; failure here fails every job
        // in the chunk identically.
        let ctx = match build_trip_context(db, trip_id) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("trip lookup failed: {e}");
                for item in *chunk {
                    let _ = record_failed(db, item, &msg);
                    failed.fetch_add(1, Ordering::Relaxed);
                }
                emit_progress_throttled(
                    app,
                    total,
                    &done,
                    &failed,
                    &last_emit,
                    EMIT_INTERVAL,
                    None,
                );
                continue;
            }
        };

        if ctx.front_sources.is_empty() {
            for item in *chunk {
                let _ = record_failed(db, item, "no segments found for trip");
                failed.fetch_add(1, Ordering::Relaxed);
            }
            emit_progress_throttled(
                app,
                total,
                &done,
                &failed,
                &last_emit,
                EMIT_INTERVAL,
                None,
            );
            continue;
        }

        // Worker pool for this trip's (tier × channel) jobs. Every
        // worker pulls the next index off `next_index` until the
        // chunk drains or cancel fires. `thread::scope` joins all
        // spawns before the outer for-loop advances, so the per-trip
        // `ctx` borrow stays live for every reader.
        let next_index = AtomicUsize::new(0);
        let ctx_ref: &TripEncodeContext = &ctx;
        thread::scope(|s| {
            for _ in 0..pool_size {
                s.spawn(|| {
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let i = next_index.fetch_add(1, Ordering::Relaxed);
                        if i >= chunk.len() {
                            break;
                        }
                        let item = &chunk[i];

                        // Top-of-loop emit so failure paths (which return
                        // ProcessOutcome::Failed without doing IO) still
                        // surface a "now working on…" event rather than
                        // hiding behind a stale current_*.
                        emit_progress_throttled(
                            app,
                            total,
                            &done,
                            &failed,
                            &last_emit,
                            EMIT_INTERVAL,
                            Some(item),
                        );

                        match process_item(
                            db,
                            cancel,
                            item,
                            ctx_ref,
                            encoder,
                            ffmpeg_path,
                            caps,
                            &output_root,
                            cpu_pool_threads,
                        ) {
                            ProcessOutcome::Done => {
                                done.fetch_add(1, Ordering::Relaxed);
                            }
                            ProcessOutcome::Failed => {
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                            ProcessOutcome::Cancelled => {
                                // Cancel flag is already set by the
                                // user; the chunk-loop's top-level
                                // check fires on the next iteration.
                                break;
                            }
                        }
                    }
                });
            }
        });
    }

    // Sweep .tmp/ if every per-job dir cleaned up. `remove_dir` is
    // empty-only, so a half-cleaned state from a process kill is
    // left alone rather than touched.
    let tmp_root = output_root.join(".tmp");
    let _ = std::fs::remove_dir(&tmp_root);

    let final_done = done.load(Ordering::Relaxed);
    let final_failed = failed.load(Ordering::Relaxed);

    // Final progress emit with closing tallies. The throttle gate
    // would otherwise swallow this — there's no next iteration to
    // unblock it.
    let _ = app.emit(
        "timelapse:progress",
        TimelapseProgressEvent {
            total,
            done: final_done,
            failed: final_failed,
            current_trip_id: None,
            current_tier: None,
            current_channel: None,
        },
    );

    let cancelled = cancel.load(Ordering::Relaxed);
    let _ = app.emit(
        "timelapse:done",
        TimelapseDoneEvent {
            total,
            done: final_done,
            failed: final_failed,
            cancelled,
        },
    );

    Ok(())
}

/// Group the flat work list into per-trip slices. The input is
/// already ordered by trip (build_work_list's outer loop is trips),
/// so we just scan for trip_id boundaries.
fn group_work_by_trip(work: &[WorkItem]) -> Vec<&[WorkItem]> {
    let mut chunks: Vec<&[WorkItem]> = Vec::new();
    if work.is_empty() {
        return chunks;
    }
    let mut start = 0usize;
    for i in 1..work.len() {
        if work[i].trip_id != work[i - 1].trip_id {
            chunks.push(&work[start..i]);
            start = i;
        }
    }
    chunks.push(&work[start..]);
    chunks
}

/// Emit a `timelapse:progress` event if the throttle interval has
/// elapsed since the previous emit. Counters are read with
/// `Relaxed`: the IPC payload is for human-paced UI, so a frame-
/// level race between done/failed updates and the emit produces
/// numbers stale by at most one increment, which the next emit
/// corrects.
fn emit_progress_throttled(
    app: &AppHandle,
    total: u64,
    done: &AtomicU64,
    failed: &AtomicU64,
    last_emit: &Mutex<Instant>,
    interval: Duration,
    item: Option<&WorkItem>,
) {
    {
        let mut guard = match last_emit.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.elapsed() < interval {
            return;
        }
        *guard = Instant::now();
    }

    let _ = app.emit(
        "timelapse:progress",
        TimelapseProgressEvent {
            total,
            done: done.load(Ordering::Relaxed),
            failed: failed.load(Ordering::Relaxed),
            current_trip_id: item.map(|i| i.trip_id.clone()),
            current_tier: item.map(|i| i.tier.as_str().to_string()),
            current_channel: item.map(|i| i.channel.as_str().to_string()),
        },
    );
}

enum ProcessOutcome {
    Done,
    Failed,
    Cancelled,
}

/// Run one (trip, tier, channel) encode end-to-end. The caller owns
/// the progress counters and increments them based on the returned
/// outcome. All DB/file side effects are confined to this function;
/// caller-side state lives in the `process_item`-free scope.
#[allow(clippy::too_many_arguments)]
fn process_item(
    db: &DbHandle,
    cancel: &CancelFlag,
    item: &WorkItem,
    ctx: &TripEncodeContext,
    encoder: Encoder,
    ffmpeg_path: &str,
    caps: &FfmpegCapabilities,
    output_root: &Path,
    cpu_pool_threads: Option<usize>,
) -> ProcessOutcome {
    if let Err(e) = mark_running(db, item) {
        eprintln!("[timelapse] mark_running failed: {e}");
        return ProcessOutcome::Failed;
    }

    // Resolve which of this channel's siblings exist. Missing positions
    // are NOT padded with black — the channel is encoded from its real
    // footage only and its curve omits the gaps; the player holds +
    // black-overlays across them. Empty `sources` ⇒ camera never records
    // this channel ⇒ skip the job.
    let resolution = match resolve_channel_sources(
        &ctx.front_sources,
        &ctx.front_durations,
        item.channel,
    ) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("sibling lookup failed: {e}");
            eprintln!(
                "[timelapse] job failed: trip={} tier={} channel={} error={}",
                item.trip_id,
                item.tier.as_str(),
                item.channel.as_str(),
                msg
            );
            let _ = record_failed(db, item, &msg);
            return ProcessOutcome::Failed;
        }
    };

    // Always create the per-job scratch dir. Two paths need it:
    // placeholder generation (PadWithBlack) and the multi-window
    // encode pipeline (curve.len() > 1). Unifying the create+sweep
    // logic up here keeps cleanup consistent across cancel and error
    // paths and means stray .tmp dirs can't accumulate.
    let scratch_dir = output_root.join(".tmp").join(format!(
        "{}_{}_{}",
        item.trip_id,
        item.tier.as_str(),
        item.channel.as_str()
    ));
    if let Err(e) = std::fs::create_dir_all(&scratch_dir) {
        let _ = std::fs::remove_dir(&scratch_dir);
        let msg = format!("scratch dir create failed: {e}");
        eprintln!(
            "[timelapse] job failed: trip={} tier={} channel={} error={}",
            item.trip_id,
            item.tier.as_str(),
            item.channel.as_str(),
            msg
        );
        let _ = record_failed(db, item, &msg);
        return ProcessOutcome::Failed;
    }

    if resolution.sources.is_empty() {
        sweep_scratch_dir(&scratch_dir);
        let msg = "no files for this channel (camera may not record it)";
        eprintln!(
            "[timelapse] job skipped: trip={} tier={} channel={} reason={}",
            item.trip_id,
            item.tier.as_str(),
            item.channel.as_str(),
            msg
        );
        let _ = record_failed(db, item, msg);
        return ProcessOutcome::Failed;
    }
    let ChannelResolution {
        sources,
        covered,
        missing_count,
    } = resolution;

    // Build the trip-level curve once, then restrict it to the ranges
    // THIS channel actually has footage for. `persist_curve` is in
    // concat-time (gappy where the camera was off) and is what the
    // player reads to hold + black-overlay across gaps. `source_curve`
    // collapses those gaps to source-time so the encoder's per-window
    // `-ss` seeks land in the gap-closed real-footage source. For a
    // full-coverage channel both equal the trip curve, so the output
    // and metadata are byte-for-byte what the old path produced.
    let trip_curve = crate::timelapse::speed_curve::build_curve(
        &ctx.windows,
        item.tier,
        ctx.total_duration_s,
    );
    let restricted = crate::timelapse::speed_curve::restrict_curve_to_coverage(
        &trip_curve,
        &covered,
    );
    // Guarantee no segment is so short at its rate that the per-window
    // NVENC encode can't open (coverage-boundary slivers / near-adjacent
    // events). Applied before deriving both the persisted and source
    // curves so they stay consistent.
    let persist_curve = crate::timelapse::speed_curve::sanitize_for_encode(
        &restricted,
        crate::timelapse::speed_curve::MIN_WINDOW_OUTPUT_S,
    );
    let curve = crate::timelapse::speed_curve::collapse_gaps(&persist_curve);

    let output_path = output_root.join(format!(
        "{}_{}_{}.mp4",
        item.trip_id,
        item.tier.as_str(),
        item.channel.as_str()
    ));

    // With black placeholders gone, every concat source is now uniform
    // real Wolf Box footage (same SPS/pix_fmt/colour), so the NVDEC +
    // scale_cuda path no longer hits the placeholder→real reinit failure
    // that forced software decode. Keep the fast GPU-only pipeline.
    let software_input = false;

    let args = EncodeArgs {
        ffmpeg_path,
        source_paths: &sources,
        output_path: &output_path,
        tier: item.tier,
        channel: item.channel,
        encoder,
        curve: &curve,
        scratch_dir: &scratch_dir,
        cpu_pool_threads,
        software_input,
    };
    let started = Instant::now();
    let encode_result = ffmpeg::encode_trip_channel(&args, cancel);
    let elapsed = started.elapsed();

    // Sweep the per-job scratch dir: placeholders, multi-window
    // source/window temp files, and any partial debris. Best-effort
    // — leaving a stray file is preferable to failing the worker
    // loop on a permissions hiccup.
    sweep_scratch_dir(&scratch_dir);

    match encode_result {
        Ok(path) => {
            // Persist the concat-time (gappy) curve — that's what the
            // player maps against. The encoder used the gap-collapsed
            // source-time curve; never persist that one.
            let curve_json = crate::timelapse::speed_curve::serialize_curve(&persist_curve);
            let output_size_bytes = std::fs::metadata(&path).ok().map(|m| m.len() as i64);
            if let Err(e) = record_done(
                db,
                item,
                &path,
                &caps.version,
                encoder.as_str(),
                // `padded_count` column now records gapped (missing)
                // segments — no black is baked, but it stays a useful
                // "how many gaps does this channel have" diagnostic.
                missing_count as i64,
                &curve_json,
                output_size_bytes,
            ) {
                eprintln!("[timelapse] record_done failed: {e}");
                return ProcessOutcome::Failed;
            }
            // Per-job timing log. The DB has created_at_ms /
            // completed_at_ms on the row already, but those are only
            // visible via direct query; this surfaces the wallclock
            // in the app log so a slow encode is greppable after the
            // fact without joining tables.
            eprintln!(
                "[timelapse] job done: trip={} tier={} channel={} elapsed={:.1}s gaps={} segments={}",
                item.trip_id,
                item.tier.as_str(),
                item.channel.as_str(),
                elapsed.as_secs_f64(),
                missing_count,
                curve.len()
            );
            ProcessOutcome::Done
        }
        Err(AppError::Internal(ref msg)) if msg == "cancelled" => {
            // Revert the row so a re-run picks this trip up.
            let _ = reset_pending(db, item);
            ProcessOutcome::Cancelled
        }
        Err(e) => {
            eprintln!(
                "[timelapse] job failed: trip={} tier={} channel={} error={}",
                item.trip_id,
                item.tier.as_str(),
                item.channel.as_str(),
                e
            );
            // Remove any stale output left at the canonical path (e.g. a
            // prior build's file that this attempt didn't overwrite before
            // failing). Otherwise `relink_present_outputs` sees the stale
            // file against this now-failed row and silently resurrects it
            // as "done" — which served the old broken rear timelapse after
            // the 16x/60x re-encode failed.
            if output_path.exists() {
                let _ = std::fs::remove_file(&output_path);
            }
            let _ = record_failed(db, item, &e.to_string());
            ProcessOutcome::Failed
        }
    }
}

/// Best-effort cleanup of a per-job scratch directory: remove every
/// file under it, then the directory itself. Used at the end of
/// `process_item` regardless of encode outcome so temp files from
/// placeholder generation, multi-window source-prep, or aborted
/// per-window encodes don't accumulate. Failures are swallowed —
/// a leftover .tmp file is a smaller problem than a worker loop
/// that won't advance because cleanup hit a transient I/O error.
fn sweep_scratch_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn mark_running(db: &DbHandle, item: &WorkItem) -> Result<(), AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::mark_running(
        &conn,
        &item.trip_id,
        item.tier.as_str(),
        item.channel.as_str(),
    )
}

#[allow(clippy::too_many_arguments)]
fn record_done(
    db: &DbHandle,
    item: &WorkItem,
    output_path: &Path,
    ffmpeg_version: &str,
    encoder_used: &str,
    padded_count: i64,
    speed_curve_json: &str,
    output_size_bytes: Option<i64>,
) -> Result<(), AppError> {
    let archive_root = db.archive_root().to_path_buf();
    // Store the path archive-relative so it survives drive remounts.
    // Falls back to the absolute string if the output somehow landed
    // outside the archive root — better to keep a usable-while-mounted
    // path than refuse to record the row at all.
    let stored_path = crate::paths::to_archive_relative(output_path, &archive_root)
        .unwrap_or_else(|_| output_path.to_string_lossy().to_string());
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::mark_done(
        &conn,
        &item.trip_id,
        item.tier.as_str(),
        item.channel.as_str(),
        &stored_path,
        ffmpeg_version,
        encoder_used,
        padded_count,
        speed_curve_json,
        output_size_bytes,
    )
}

fn record_failed(db: &DbHandle, item: &WorkItem, message: &str) -> Result<(), AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::mark_failed(
        &conn,
        &item.trip_id,
        item.tier.as_str(),
        item.channel.as_str(),
        message,
    )
}

fn reset_pending(db: &DbHandle, item: &WorkItem) -> Result<(), AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::reset_to_pending(
        &conn,
        &item.trip_id,
        item.tier.as_str(),
        item.channel.as_str(),
    )
}

/// Per-trip state shared across the (tier × channel) jobs of one trip.
/// Built once before the worker pool spins up; every worker reads it
/// concurrently for the duration of the chunk.
struct TripEncodeContext {
    front_sources: Vec<String>,
    /// Per-segment duration (parallel to `front_sources`). Used to
    /// generate correctly-sized black placeholders when a sibling is
    /// genuinely missing — the placeholder matches the front segment's
    /// duration so concat-timeline stays aligned across channels.
    front_durations: Vec<f64>,
    windows: Vec<EventWindow>,
    total_duration_s: f64,
}

fn build_trip_context(
    db: &DbHandle,
    trip_id: &str,
) -> Result<TripEncodeContext, AppError> {
    let segments = trip_segment_info(db, trip_id)?;
    let total_duration_s: f64 = segments.iter().map(|s| s.duration_s).sum();
    let front_sources: Vec<String> =
        segments.iter().map(|s| s.master_path.clone()).collect();
    let front_durations: Vec<f64> = segments.iter().map(|s| s.duration_s).collect();
    let stitched = stitch_trip_gps(&segments);

    // Persist trip-stitched GPS so map + speed graph survive a future
    // "Delete originals". Idempotent — has_current() skips re-writes
    // when an existing row is already at/above the current parser
    // version. Persistence failures don't fail the encode; we just
    // leave the row stale and pick it up on the next pass.
    if !segments.is_empty() {
        if let Ok(conn) = db.lock() {
            let needs_write = !crate::db::trip_gps::has_current(
                &conn,
                trip_id,
                crate::gps::GPS_PARSER_VERSION,
            )
            .unwrap_or(false);
            if needs_write {
                if let Err(e) = crate::db::trip_gps::upsert(
                    &conn,
                    trip_id,
                    &stitched,
                    crate::gps::GPS_PARSER_VERSION,
                ) {
                    eprintln!("[timelapse] trip_gps upsert failed for {trip_id}: {e}");
                }
            }
        }
    }

    let windows = events::detect_events(&stitched);
    Ok(TripEncodeContext {
        front_sources,
        front_durations,
        windows,
        total_duration_s,
    })
}

/// One segment's worth of info needed to build the concat list and
/// stitch GPS across the trip.
#[derive(Debug, Clone)]
pub(crate) struct SegmentInfo {
    pub(crate) master_path: String,
    pub(crate) duration_s: f64,
    pub(crate) camera_kind: CameraKind,
}

fn camera_kind_from_str(s: &str) -> CameraKind {
    match s {
        "wolfBox" => CameraKind::WolfBox,
        "thinkware" => CameraKind::Thinkware,
        "miltona" => CameraKind::Miltona,
        _ => CameraKind::Generic,
    }
}

/// All segments of a trip, ordered by start time, with the info
/// needed for both the concat list and GPS stitching.
///
/// Post-migration the `master_path` column holds archive-relative
/// forward-slash paths; the timelapse pipeline (ffmpeg and
/// `find_sibling_file`) needs absolute paths on the local filesystem,
/// so each row is rejoined to `db.archive_root()` here.
/// `from_archive_relative` returns the stored value unchanged when it's
/// already absolute, so pre-migration rows still resolve correctly.
pub(crate) fn trip_segment_info(db: &DbHandle, trip_id: &str) -> Result<Vec<SegmentInfo>, AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    // Tombstones have no scannable file — exclude them so the concat
    // list and GPS stitcher only ever see real originals.
    let mut stmt = conn.prepare(
        "SELECT master_path, duration_s, camera_kind
         FROM segments
         WHERE trip_id = ?1 AND is_tombstone = 0
         ORDER BY start_time_ms ASC",
    )?;
    let rows = stmt.query_map(params![trip_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (rel_path, duration, kind) = r?;
        let abs_path = crate::paths::from_archive_relative(&rel_path, &archive_root);
        out.push(SegmentInfo {
            master_path: abs_path.to_string_lossy().into_owned(),
            duration_s: duration,
            camera_kind: camera_kind_from_str(&kind),
        });
    }
    Ok(out)
}

/// Append one segment's GPS points to the stitched track, remapping each
/// to concat-timeline (`cursor + local offset`).
///
/// Points whose local `t_offset_s` runs past the segment's video
/// duration are dropped. Wolf Box parking-mode clips embed GPS for the
/// whole parked interval (~90 min) into a ~180s video: the `gpmd` track
/// carries thousands of samples while the video track stays short. Only
/// the points within the video duration line up with the frames the
/// concat actually shows; keeping the rest pushes the segment's offsets
/// far past `cursor`, so the *next* segment's points appear to jump
/// backwards in concat time. That non-monotonic timeline is what desyncs
/// the map marker (which advances smoothly) from the video (parked).
fn append_segment_points(
    out: &mut Vec<GpsPoint>,
    cursor: f64,
    duration_s: f64,
    points: Vec<GpsPoint>,
) {
    for p in points {
        if p.t_offset_s > duration_s {
            continue;
        }
        out.push(GpsPoint {
            t_offset_s: cursor + p.t_offset_s,
            ..p
        });
    }
}

/// Stitch GPS across all segments of a trip, remapping each point's
/// `t_offset_s` to concat-timeline (cumulative duration of prior
/// segments). Segments with no GPS contribute no points but still
/// advance the time cursor. Failed extractions are treated as empty
/// — a missing GPS trace shouldn't fail the whole encode.
pub(crate) fn stitch_trip_gps(segments: &[SegmentInfo]) -> Vec<GpsPoint> {
    let sources: Vec<gps::TripGpsSegmentSource> = segments
        .iter()
        .map(|seg| {
            let mut paths = vec![seg.master_path.clone()];
            for channel in [Channel::Interior, Channel::Rear] {
                if let Ok(Some(path)) = resolve_sibling_path(&seg.master_path, channel) {
                    paths.push(path.to_string_lossy().into_owned());
                }
            }
            gps::TripGpsSegmentSource {
                duration_s: seg.duration_s,
                camera_kind: seg.camera_kind,
                paths,
            }
        })
        .collect();
    gps::extract_best_trip_gps(&sources)
}

/// Resolve the sibling channel file for a front-channel path by
/// delegating to the scan layer's fuzzy matcher. The scanner accepts
/// 1-2 second per-channel timestamp skew (empirical Wolf Box behavior)
/// via `SEGMENT_FUZZY_WINDOW_S` — a naive filename letter-swap misses
/// those siblings, which caused the false-positive "missing files"
/// diagnosis that preceded this refactor.
///
/// Returns `Ok(Some)` if the sibling exists on disk, `Ok(None)` if it
/// doesn't (the channel is genuinely absent for that segment), and
/// `Err` only for IO-level problems reading the parent directory.
fn resolve_sibling_path(
    front_path: &str,
    target: Channel,
) -> Result<Option<std::path::PathBuf>, AppError> {
    crate::scan::grouping::find_sibling_file(
        std::path::Path::new(front_path),
        target.label(),
    )
}

/// Resolution of one channel's sources for a trip: the real sibling
/// files that exist (gaps omitted, in segment order), the maximal
/// concat-time ranges they cover, and how many segments were missing.
///
/// When a camera is off for part of a trip the missing positions are
/// NOT padded with black. The channel is encoded from its real footage
/// only and its persisted curve omits the missing concat-ranges; the
/// player holds + black-overlays the channel across those gaps at
/// playback time. No baked black means an alignment/coverage fix never
/// needs a re-encode — only the curve metadata is regenerated.
///
/// `sources` empty means the camera never records this channel
/// (single-channel Miltona, Thinkware without rear, etc.) — the caller
/// skips the (trip, channel) job gracefully. A full-coverage channel
/// (the Front master always is, since `front_sources` *are* the trip's
/// segments) yields a single `[0, total]` covered range.
#[derive(Debug, Default)]
pub struct ChannelResolution {
    pub sources: Vec<String>,
    pub covered: Vec<(f64, f64)>,
    pub missing_count: usize,
}

/// Resolve sources for one (trip, channel) pair. Walks the trip's
/// master_paths and probes the requested channel's sibling at each
/// position, collecting the real files that exist and the maximal
/// concat-time ranges they cover (runs of consecutive present
/// siblings). The concat cursor advances by each segment's duration
/// whether or not the sibling exists, so covered ranges line up with
/// the trip's concat timeline.
/// A present sibling counts as real coverage only if its actual video
/// duration is at least this fraction of the front segment's duration.
/// Wolf Box occasionally writes a degenerate stub sibling (e.g. a 0.1s
/// rear clip alongside a 180s front segment — a truncated/aborted
/// recording). Crediting that stub the *front's* full span inflates the
/// channel's coverage far past its real footage, so the source-time
/// curve overruns the actual (gap-closed) source file and tail windows
/// seek past EOF — which makes the per-window NVENC encode fail to open
/// ("hw_frames_ctx must be set" / -22). Treating such a stub as a gap
/// keeps coverage honest. 0.5 cleanly separates legitimate clock skew
/// (~0.99) and matching tiny parking clips from a true stub (~0.0006).
const MIN_COVERAGE_FRACTION: f64 = 0.5;

/// Decide whether a found sibling counts as coverage. `real_dur` is the
/// sibling's probed video duration, or `None` if it couldn't be probed
/// (corrupt/unreadable header) — in which case we keep the historical
/// "exists ⇒ present" behavior rather than second-guessing. A
/// successfully-probed sibling shorter than `MIN_COVERAGE_FRACTION` of
/// its front segment is rejected as a degenerate stub.
fn sibling_counts_as_covered(real_dur: Option<f64>, front_dur: f64) -> bool {
    match real_dur {
        Some(d) => d >= front_dur * MIN_COVERAGE_FRACTION,
        None => true,
    }
}

fn resolve_channel_sources(
    front_sources: &[String],
    front_durations: &[f64],
    channel: Channel,
) -> Result<ChannelResolution, AppError> {
    debug_assert_eq!(front_sources.len(), front_durations.len());

    let mut res = ChannelResolution::default();
    let mut cursor = 0.0f64;
    let mut run_start: Option<f64> = None;

    for (path, &duration) in front_sources.iter().zip(front_durations.iter()) {
        let seg_start = cursor;
        // A sibling that exists but is a degenerate stub (probed duration
        // far below the front segment) is treated as missing — see
        // `MIN_COVERAGE_FRACTION`. Probe failures keep "exists ⇒ present".
        let present = match resolve_sibling_path(path, channel)? {
            Some(sibling) => {
                let real_dur =
                    crate::metadata::mp4_probe::probe(&sibling).map(|m| m.duration_s).ok();
                if sibling_counts_as_covered(real_dur, duration) {
                    Some(sibling)
                } else {
                    None
                }
            }
            None => None,
        };
        match present {
            Some(sibling) => {
                res.sources.push(sibling.to_string_lossy().into_owned());
                if run_start.is_none() {
                    run_start = Some(seg_start);
                }
            }
            None => {
                res.missing_count += 1;
                // A gap closes the current covered run at the end of the
                // previous present segment (== this segment's start).
                if let Some(rs) = run_start.take() {
                    res.covered.push((rs, seg_start));
                }
            }
        }
        cursor = seg_start + duration;
    }
    if let Some(rs) = run_start.take() {
        res.covered.push((rs, cursor));
    }
    Ok(res)
}

fn build_work_list(
    db: &DbHandle,
    trip_ids: Option<&[String]>,
    tiers: &[Tier],
    channels: &[Channel],
    scope: JobScope,
) -> Result<Vec<WorkItem>, AppError> {
    // Resolve trip list. `None` means "every trip in the library".
    let trips: Vec<String> = if let Some(ids) = trip_ids {
        ids.to_vec()
    } else {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare("SELECT id FROM trips ORDER BY start_time_ms ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out
    };

    let mut work = Vec::new();
    let mut skipped_archive_only = 0u64;
    for trip_id in &trips {
        // Archive-only trip guard. Trips whose only segments are
        // tombstones (or whose segment rows were GC'd because the
        // originals vanished) have no source files to encode from.
        // Enqueuing them would call `upsert_pending`, which nulls the
        // existing row's `output_path`; the encode then fails with
        // "no segments found for trip" and the on-disk timelapse
        // becomes invisible in the UI even though the file is still
        // there. Skip entirely so the existing row stays intact.
        if !trip_has_source_segments(db, trip_id)? {
            skipped_archive_only += 1;
            continue;
        }
        for tier in tiers {
            for channel in channels {
                if should_enqueue(db, trip_id, *tier, *channel, scope)? {
                    // Create or reset the row so progress tracking has
                    // a home for each unit. `upsert_pending` resets
                    // already-done rows when scope says to rebuild.
                    {
                        let conn = db.lock().map_err(|_| {
                            AppError::Internal("db mutex poisoned".into())
                        })?;
                        db::timelapse_jobs::upsert_pending(
                            &conn,
                            trip_id,
                            tier.as_str(),
                            channel.as_str(),
                        )?;
                    }
                    work.push(WorkItem {
                        trip_id: trip_id.clone(),
                        tier: *tier,
                        channel: *channel,
                    });
                }
            }
        }
    }
    if skipped_archive_only > 0 {
        eprintln!(
            "[timelapse] build_work_list: skipped {skipped_archive_only} archive-only trip(s) (no source segments — existing outputs left intact)"
        );
    }
    Ok(work)
}

/// True when at least one non-tombstone segment row exists for the
/// trip. Mirrors the filter that `trip_segment_info` applies so the
/// guard in `build_work_list` matches the worker's source-resolution
/// semantics exactly.
fn trip_has_source_segments(db: &DbHandle, trip_id: &str) -> Result<bool, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM segments
         WHERE trip_id = ?1 AND is_tombstone = 0",
        params![trip_id],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn should_enqueue(
    db: &DbHandle,
    trip_id: &str,
    tier: Tier,
    channel: Channel,
    scope: JobScope,
) -> Result<bool, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let existing =
        db::timelapse_jobs::get(&conn, trip_id, tier.as_str(), channel.as_str())?;
    Ok(match (scope, existing) {
        // NewOnly = "anything not already completed successfully".
        // That means: no row (fresh work), pending (waiting), or
        // running (stale/orphan from a hard exit — cleanup_stale_jobs
        // should have reset these but be defensive). This lets the
        // user cancel a run and click Start again to resume the
        // remaining work without having to choose RebuildAll and
        // re-encode what already finished.
        (JobScope::NewOnly, None) => true,
        (JobScope::NewOnly, Some(row)) => row.status != db::timelapse_jobs::STATUS_DONE,
        (JobScope::FailedOnly, Some(row)) => row.status == db::timelapse_jobs::STATUS_FAILED,
        (JobScope::FailedOnly, None) => false,
        (JobScope::RebuildAll, _) => true,
    })
}

/// `<archive_root>/Timelapses/`, created if missing. With the per-archive
/// DB now living inside the archive, the archive root is implicit (it's
/// the DB's grandparent) — no more cache key, no more discovery walk.
fn resolve_output_root(db: &DbHandle) -> Result<PathBuf, AppError> {
    let out = db.archive_root().join("Timelapses");
    std::fs::create_dir_all(&out)?;
    Ok(out)
}

/// Result of probing the segments table for a library root.
///
/// `Library` is the "structured" answer — the segment path lived under
/// a `Videos/` directory, so we know its parent is a real library root
/// laid out by the import pipeline.
///
/// `SegmentParent` is the fallback: the segment was scanned in place
/// from an arbitrary folder.
///
/// Used only by the per-archive migration to suggest an archive root
/// from the legacy DB's absolute `master_path` values. Once the user
/// confirms the root, the per-archive DB stores paths *relative* to it
/// and discovery is no longer needed at runtime.
#[allow(dead_code)] // wired up by migration_v2 in the next change.
pub(crate) enum DiscoveredRoot {
    Library(PathBuf),
    SegmentParent(PathBuf),
}

#[allow(dead_code)] // wired up by migration_v2 in the next change.
pub(crate) fn discover_library_root(conn: &Connection) -> Result<DiscoveredRoot, AppError> {
    let sample: Option<String> = conn
        .query_row("SELECT master_path FROM segments LIMIT 1", [], |r| r.get(0))
        .ok();
    let Some(sample_path) = sample else {
        return Err(AppError::Internal(
            "cannot derive library root: no segments in DB".into(),
        ));
    };
    let p = PathBuf::from(sample_path);
    // Walk up until we find a "Videos" directory; its parent is the root.
    for ancestor in p.ancestors() {
        if ancestor.file_name().map(|n| n == "Videos").unwrap_or(false) {
            if let Some(parent) = ancestor.parent() {
                return Ok(DiscoveredRoot::Library(parent.to_path_buf()));
            }
        }
    }
    // No Videos/ ancestor — segment was scanned in place. Drop
    // Timelapses/ next to the source MP4s.
    let parent = p.parent().ok_or_else(|| {
        AppError::Internal(format!(
            "segment path has no parent directory: {}",
            p.display()
        ))
    })?;
    Ok(DiscoveredRoot::SegmentParent(parent.to_path_buf()))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use std::env::temp_dir;
    use std::fs;

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        // Combining PID with a per-test tag plus a monotonic counter keeps
        // parallel test runs from tripping over each other in %TEMP%.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        temp_dir().join(format!("tripviewer-{tag}-{}-{}", std::process::id(), n))
    }

    fn insert_segment_with_path(db: &DbHandle, master_path: &str) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO segments (id, trip_id, start_time_ms, duration_s,
                master_path, is_event, camera_kind, gps_supported, last_seen_ms)
             VALUES ('seg', 'trip', 0, 60.0, ?1, 0, 'wolfbox', 1, 0)",
            params![master_path],
        )
        .unwrap();
    }

    fn gp(t: f64) -> GpsPoint {
        GpsPoint {
            t_offset_s: t,
            lat: 0.0,
            lon: 0.0,
            speed_mps: 0.0,
            heading_deg: 0.0,
            altitude_m: 0.0,
            fix_ok: true,
        }
    }

    #[test]
    fn append_segment_points_trims_to_duration_and_offsets() {
        let mut out = Vec::new();
        // A parking-mode clip: 180s video, but GPS samples run out to
        // 5399s (whole parked interval). Only points <= 180 should land.
        let parking: Vec<GpsPoint> = (0..=5399).map(|s| gp(s as f64)).collect();
        append_segment_points(&mut out, 0.0, 180.0, parking);
        assert_eq!(out.len(), 181, "kept points beyond the video duration");
        assert_eq!(out.first().unwrap().t_offset_s, 0.0);
        assert_eq!(out.last().unwrap().t_offset_s, 180.0);

        // Next segment starts at cursor=180; its points must not appear
        // before the trimmed tail of the previous one.
        append_segment_points(&mut out, 180.0, 60.0, vec![gp(0.0), gp(30.0), gp(60.0)]);
        let monotonic = out.windows(2).all(|w| w[1].t_offset_s >= w[0].t_offset_s);
        assert!(monotonic, "stitched concat-timeline went backwards");
        assert_eq!(out.last().unwrap().t_offset_s, 240.0);
    }

    #[test]
    fn discover_library_root_uses_videos_ancestor_when_present() {
        let db = open_in_memory().unwrap();
        // SD-card / folder-import layout: <root>/Videos/<file>
        insert_segment_with_path(&db, "/library/Videos/2026_04_12_132511_00_F.MP4");
        let conn = db.lock().unwrap();
        match discover_library_root(&conn).unwrap() {
            DiscoveredRoot::Library(p) => {
                assert_eq!(p, PathBuf::from("/library"));
            }
            DiscoveredRoot::SegmentParent(p) => {
                panic!("expected Library, got SegmentParent({})", p.display())
            }
        }
    }

    #[test]
    fn discover_library_root_falls_back_to_segment_parent_when_no_videos_ancestor() {
        let db = open_in_memory().unwrap();
        // scan_folder layout: arbitrary path, no Videos/ ancestor.
        insert_segment_with_path(
            &db,
            "/Users/chrisl8/Dashcam Tests/Wolfbox Example/2026_04_12_132511_00_F.MP4",
        );
        let conn = db.lock().unwrap();
        match discover_library_root(&conn).unwrap() {
            DiscoveredRoot::SegmentParent(p) => {
                assert_eq!(
                    p,
                    PathBuf::from("/Users/chrisl8/Dashcam Tests/Wolfbox Example")
                );
            }
            DiscoveredRoot::Library(p) => {
                panic!("expected SegmentParent, got Library({})", p.display())
            }
        }
    }

    #[test]
    fn resolve_output_root_uses_archive_root() {
        let scan_dir = unique_dir("scan-fallback");
        let _ = fs::remove_dir_all(&scan_dir);
        fs::create_dir_all(&scan_dir).unwrap();
        // Archive root is now bundled into the DB handle — no more
        // segment-walking discovery; the DB simply lives at
        // <archive>/.tripviewer/tripviewer.db so the parent of its
        // parent is the archive root.
        let db = crate::db::open_in_memory_with_root(&scan_dir).unwrap();

        let out = resolve_output_root(&db).unwrap();
        assert_eq!(out, scan_dir.join("Timelapses"));
        assert!(out.is_dir(), "Timelapses/ directory should be created");

        let _ = fs::remove_dir_all(&scan_dir);
    }

    #[test]
    fn resolve_sibling_finds_file_at_same_timestamp() {
        let dir = unique_dir("sibling-same");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_04_22_103001_00_F.MP4");
        let interior = dir.join("2026_04_22_103001_00_I.MP4");
        fs::write(&front, b"").unwrap();
        fs::write(&interior, b"").unwrap();

        let got = resolve_sibling_path(front.to_str().unwrap(), Channel::Interior)
            .unwrap();
        assert_eq!(got, Some(interior));
        let no_rear = resolve_sibling_path(front.to_str().unwrap(), Channel::Rear)
            .unwrap();
        assert!(no_rear.is_none(), "rear doesn't exist → None");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_sibling_finds_file_with_minus_1s_timestamp_skew() {
        // This is the March 22 4:41 PM case: rear at 164127, front at
        // 164128. The old naive swap missed it; the new matcher honors
        // the scanner's SEGMENT_FUZZY_WINDOW_S.
        let dir = unique_dir("sibling-minus1");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_03_22_164128_02_F.MP4");
        let rear = dir.join("2026_03_22_164127_02_R.MP4");
        fs::write(&front, b"").unwrap();
        fs::write(&rear, b"").unwrap();

        let got = resolve_sibling_path(front.to_str().unwrap(), Channel::Rear)
            .unwrap();
        assert_eq!(got.as_deref(), Some(rear.as_path()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_sibling_finds_file_with_plus_2s_timestamp_skew() {
        let dir = unique_dir("sibling-plus2");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_03_22_164128_02_F.MP4");
        let rear = dir.join("2026_03_22_164130_02_R.MP4");
        fs::write(&front, b"").unwrap();
        fs::write(&rear, b"").unwrap();

        let got = resolve_sibling_path(front.to_str().unwrap(), Channel::Rear)
            .unwrap();
        assert_eq!(got.as_deref(), Some(rear.as_path()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_sibling_rejects_file_4s_out_of_window() {
        // 4 s > SEGMENT_FUZZY_WINDOW_S (3 s) — don't match.
        let dir = unique_dir("sibling-outofwindow");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_03_22_164128_02_F.MP4");
        let too_far = dir.join("2026_03_22_164132_02_R.MP4");
        fs::write(&front, b"").unwrap();
        fs::write(&too_far, b"").unwrap();

        let got = resolve_sibling_path(front.to_str().unwrap(), Channel::Rear)
            .unwrap();
        assert!(got.is_none(), "4 s delta is outside the fuzzy window");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_sibling_respects_event_mode() {
        // An event_mode=00 front should NOT match an event_mode=02 rear
        // at the same timestamp — the scanner treats them as separate
        // segments, so the timelapse resolver must too.
        let dir = unique_dir("sibling-evtmode");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_03_22_164128_00_F.MP4");
        let rear_event = dir.join("2026_03_22_164128_02_R.MP4");
        fs::write(&front, b"").unwrap();
        fs::write(&rear_event, b"").unwrap();

        let got = resolve_sibling_path(front.to_str().unwrap(), Channel::Rear)
            .unwrap();
        assert!(got.is_none(), "event mode mismatch must block match");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sibling_coverage_rejects_degenerate_stub_keeps_skew_and_unprobed() {
        // The May 18 bug: a 0.1s rear stub against a 180s front segment.
        assert!(!sibling_counts_as_covered(Some(0.1), 180.0));
        // Legitimate clock skew (~0.5% short) stays covered.
        assert!(sibling_counts_as_covered(Some(179.0), 180.0));
        // Matching tiny parking clips stay covered.
        assert!(sibling_counts_as_covered(Some(0.3), 0.4));
        // Right at the half-duration boundary counts as covered.
        assert!(sibling_counts_as_covered(Some(90.0), 180.0));
        assert!(!sibling_counts_as_covered(Some(89.0), 180.0));
        // Unprobeable sibling (corrupt header / empty test file) keeps
        // the historical "exists ⇒ present" behavior.
        assert!(sibling_counts_as_covered(None, 180.0));
    }

    #[test]
    fn resolve_channel_sources_front_complete_when_all_masters_are_front() {
        // The common case: master_paths are all F files; the F job's
        // sibling lookup finds each master as its own F sibling and
        // we get Complete back with the same paths.
        let dir = unique_dir("front-complete");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front_a = dir.join("2026_04_22_103001_00_F.MP4");
        let front_b = dir.join("2026_04_22_103300_00_F.MP4");
        fs::write(&front_a, b"").unwrap();
        fs::write(&front_b, b"").unwrap();

        let sources = vec![
            front_a.to_string_lossy().to_string(),
            front_b.to_string_lossy().to_string(),
        ];
        let durations = vec![180.0, 180.0];
        let r = resolve_channel_sources(&sources, &durations, Channel::Front).unwrap();
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.missing_count, 0);
        assert!(r.sources.iter().any(|s| s.ends_with("103001_00_F.MP4")));
        assert!(r.sources.iter().any(|s| s.ends_with("103300_00_F.MP4")));
        // Full coverage → a single [0, total] range.
        assert_eq!(r.covered, vec![(0.0, 360.0)]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_front_gaps_when_a_master_is_not_front() {
        // The 9:43 AM trip case: the first segment's master is a Rear
        // file (no F existed at that timestamp). The F job leaves that
        // slot as a coverage gap (no source) rather than concatenating
        // the wrong-channel rear file into the F output — the player
        // black-overlays the gap at playback time.
        let dir = unique_dir("front-gaps-when-master-is-r");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Segment 1: only a Rear file on disk.
        let rear_only = dir.join("2026_03_23_094334_00_R.MP4");
        // Segment 2: Front file on disk (Interior/Rear may also exist
        // but we only care about Front for this test).
        let front_b = dir.join("2026_03_23_094634_00_F.MP4");
        fs::write(&rear_only, b"").unwrap();
        fs::write(&front_b, b"").unwrap();

        let sources = vec![
            rear_only.to_string_lossy().to_string(),
            front_b.to_string_lossy().to_string(),
        ];
        let durations = vec![180.0, 180.0];
        // Segment 0 has no F on disk → coverage gap; segment 1 is real.
        let r = resolve_channel_sources(&sources, &durations, Channel::Front).unwrap();
        assert_eq!(r.missing_count, 1);
        assert_eq!(r.sources.len(), 1);
        assert!(r.sources[0].ends_with("094634_00_F.MP4"));
        // Only segment 1 (concat [180, 360)) is covered; [0, 180) is the gap.
        assert_eq!(r.covered, vec![(180.0, 360.0)]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_gaps_for_partial_miss() {
        let dir = unique_dir("gapmiss");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front_a = dir.join("2026_04_22_103001_00_F.MP4");
        let interior_a = dir.join("2026_04_22_103001_00_I.MP4");
        let front_b = dir.join("2026_04_22_103300_00_F.MP4");
        // deliberately NO interior_b on disk (not even within fuzzy window).
        fs::write(&front_a, b"").unwrap();
        fs::write(&interior_a, b"").unwrap();
        fs::write(&front_b, b"").unwrap();

        let sources = vec![
            front_a.to_string_lossy().to_string(),
            front_b.to_string_lossy().to_string(),
        ];
        let durations = vec![180.0, 175.5];
        // Segment 0 (interior present) is covered; segment 1 (interior
        // missing) is a gap. durations = [180, 175.5].
        let r = resolve_channel_sources(&sources, &durations, Channel::Interior).unwrap();
        assert_eq!(r.missing_count, 1);
        assert_eq!(r.sources.len(), 1);
        assert!(r.sources[0].ends_with("_I.MP4"));
        assert_eq!(r.covered, vec![(0.0, 180.0)]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_empty_when_camera_never_records() {
        let dir = unique_dir("allmiss");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_04_22_103001_00_F.MP4");
        fs::write(&front, b"").unwrap();
        // No rear file at all.

        let sources = vec![front.to_string_lossy().to_string()];
        let durations = vec![180.0];
        let r = resolve_channel_sources(&sources, &durations, Channel::Rear).unwrap();
        assert!(r.sources.is_empty(), "camera never records this channel");
        assert!(r.covered.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_complete_when_all_siblings_present_even_with_skew() {
        let dir = unique_dir("complete-skew");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front_a = dir.join("2026_04_22_103001_00_F.MP4");
        let rear_a = dir.join("2026_04_22_103000_00_R.MP4"); // -1 s skew
        let front_b = dir.join("2026_04_22_103300_00_F.MP4");
        let rear_b = dir.join("2026_04_22_103302_00_R.MP4"); // +2 s skew
        fs::write(&front_a, b"").unwrap();
        fs::write(&rear_a, b"").unwrap();
        fs::write(&front_b, b"").unwrap();
        fs::write(&rear_b, b"").unwrap();

        let sources = vec![
            front_a.to_string_lossy().to_string(),
            front_b.to_string_lossy().to_string(),
        ];
        let durations = vec![180.0, 180.0];
        let r = resolve_channel_sources(&sources, &durations, Channel::Rear).unwrap();
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.missing_count, 0);
        assert!(r.sources.iter().any(|s| s.ends_with("2026_04_22_103000_00_R.MP4")));
        assert!(r.sources.iter().any(|s| s.ends_with("2026_04_22_103302_00_R.MP4")));
        assert_eq!(r.covered, vec![(0.0, 360.0)]);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Regression for the upsert_pending bug. An archive-only trip
    /// (only tombstone segments, but a `done` timelapse_jobs row
    /// pointing at an on-disk MP4) must be skipped entirely by
    /// `build_work_list` so the existing row's `output_path` isn't
    /// nulled out. Before the fix, RebuildAll would enqueue the trip,
    /// call `upsert_pending` (clearing `output_path`), the encode
    /// would then fail with "no segments found for trip", and the
    /// perfectly good on-disk timelapse would become invisible.
    #[test]
    fn build_work_list_skips_archive_only_trips_under_rebuild_all() {
        let scan_dir = unique_dir("buildlist-archive-only");
        let _ = fs::remove_dir_all(&scan_dir);
        fs::create_dir_all(&scan_dir).unwrap();
        let db = crate::db::open_in_memory_with_root(&scan_dir).unwrap();

        let trip_id = "archive-only-trip";
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind,
                    gps_supported, last_seen_ms)
                 VALUES (?1, 1000, 2000, 'wolfBox', 1, 0)",
                params![trip_id],
            )
            .unwrap();
            // A tombstone segment — present in the table but excluded
            // from `trip_segment_info` and from the new guard's count.
            conn.execute(
                "INSERT INTO segments (id, trip_id, start_time_ms, duration_s,
                    master_path, is_event, camera_kind, gps_supported, last_seen_ms,
                    is_tombstone)
                 VALUES ('seg-tomb', ?1, 1000, 60.0, '', 0, 'wolfbox', 1, 0, 1)",
                params![trip_id],
            )
            .unwrap();
            // Pre-existing done row — must survive the call untouched.
            db::timelapse_jobs::upsert_pending(&conn, trip_id, "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn,
                trip_id,
                "8x",
                "F",
                "Timelapses/archive-only-trip_8x_F.mp4",
                "7.0",
                "hevc_nvenc",
                0,
                "[]",
                Some(42),
            )
            .unwrap();
        }

        let work = build_work_list(
            &db,
            None,
            &[Tier::Tier8x],
            &[Channel::Front],
            JobScope::RebuildAll,
        )
        .unwrap();
        assert!(
            work.is_empty(),
            "archive-only trip must contribute no work items"
        );

        // Critical: output_path is still pointing at the on-disk file.
        let conn = db.lock().unwrap();
        let row = db::timelapse_jobs::get(&conn, trip_id, "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, db::timelapse_jobs::STATUS_DONE);
        assert_eq!(
            row.output_path.as_deref(),
            Some("Timelapses/archive-only-trip_8x_F.mp4"),
            "RebuildAll over an archive-only trip must NOT null output_path"
        );

        drop(conn);
        let _ = fs::remove_dir_all(&scan_dir);
    }

    /// Trips with at least one live (non-tombstone) segment must still
    /// be enqueued normally under RebuildAll — the guard is precise to
    /// the archive-only case.
    #[test]
    fn build_work_list_enqueues_trips_with_live_segments() {
        let scan_dir = unique_dir("buildlist-live");
        let _ = fs::remove_dir_all(&scan_dir);
        fs::create_dir_all(&scan_dir).unwrap();
        let db = crate::db::open_in_memory_with_root(&scan_dir).unwrap();

        let trip_id = "live-trip";
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind,
                    gps_supported, last_seen_ms)
                 VALUES (?1, 1000, 2000, 'wolfBox', 1, 0)",
                params![trip_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO segments (id, trip_id, start_time_ms, duration_s,
                    master_path, is_event, camera_kind, gps_supported, last_seen_ms,
                    is_tombstone)
                 VALUES ('seg-live', ?1, 1000, 60.0,
                         'Videos/2026_04_22_103001_00_F.MP4', 0, 'wolfbox', 1, 0, 0)",
                params![trip_id],
            )
            .unwrap();
        }

        let work = build_work_list(
            &db,
            None,
            &[Tier::Tier8x],
            &[Channel::Front],
            JobScope::RebuildAll,
        )
        .unwrap();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].trip_id, trip_id);

        let _ = fs::remove_dir_all(&scan_dir);
    }
}
