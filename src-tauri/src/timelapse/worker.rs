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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::params;
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

    let mut done: u64 = 0;
    let mut failed: u64 = 0;
    let mut last_emit = Instant::now();
    const EMIT_INTERVAL: Duration = Duration::from_millis(250);

    // Per-trip cache. The GPS stitch + event detection is the same for
    // every (tier, channel) unit of the same trip, so we compute it
    // once when the trip_id changes and reuse it for the rest. The
    // work list is grouped by trip already (build_work_list's outer
    // loop is trips) so this stays hot for adjacent items.
    let mut cached: Option<TripEncodeContext> = None;

    for item in &work {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        // Refresh the cache if we've moved to a new trip.
        if cached.as_ref().map(|c| c.trip_id.as_str()) != Some(item.trip_id.as_str()) {
            cached = match build_trip_context(db, &item.trip_id) {
                Ok(ctx) => Some(ctx),
                Err(e) => {
                    failed += 1;
                    record_failed(db, item, &format!("trip lookup failed: {e}"))?;
                    continue;
                }
            };
        }
        let ctx = cached.as_ref().expect("cache was just populated");

        if ctx.front_sources.is_empty() {
            failed += 1;
            record_failed(db, item, "no segments found for trip")?;
            continue;
        }

        // Interior/Rear outcomes: Complete → encode as-is.
        // CameraDoesNotRecord → skip (single-channel cameras).
        // PadWithBlack → generate placeholder MP4s for the missing
        // positions so the concat stream stays the expected duration
        // and stays in sync with the front/interior channels.
        let resolution = match resolve_channel_sources(
            &ctx.front_sources,
            &ctx.front_durations,
            item.channel,
        ) {
            Ok(r) => r,
            Err(e) => {
                record_failed(
                    db,
                    item,
                    &format!("sibling lookup failed: {e}"),
                )?;
                failed += 1;
                continue;
            }
        };

        // Scratch placeholder files — cleaned up at the end of the
        // iteration regardless of encode outcome.
        let mut scratch_files: Vec<PathBuf> = Vec::new();

        let (sources, padded_count) = match resolution {
            SiblingResolution::Complete(v) => (v, 0usize),
            SiblingResolution::CameraDoesNotRecord => {
                record_failed(
                    db,
                    item,
                    "no files for this channel (camera may not record it)",
                )?;
                failed += 1;
                continue;
            }
            SiblingResolution::PadWithBlack {
                entries,
                missing_count,
                reference_sibling,
            } => {
                // Probe one real sibling to match codec/resolution/fps.
                // Concat demuxer is strict — mismatched params get the
                // placeholder rejected at encode time.
                let meta = match crate::metadata::mp4_probe::probe(
                    Path::new(&reference_sibling),
                ) {
                    Ok(m) => m,
                    Err(e) => {
                        record_failed(
                            db,
                            item,
                            &format!(
                                "failed to probe reference sibling \
                                 {reference_sibling} to size black placeholders: {e}"
                            ),
                        )?;
                        failed += 1;
                        continue;
                    }
                };
                let width = meta.width.max(16);
                let height = meta.height.max(16);
                // Simple integer fps. Dashcam footage is almost always
                // 30/1; NTSC-ish 30000/1001 rounds to 30 which the
                // concat demuxer accepts via rate-tolerant muxing.
                let fps = (meta.fps_num / meta.fps_den.max(1)).max(1);

                // Probe the reference sibling for pix_fmt + color tags
                // so the placeholders match the real files' decoded
                // frame parameters. Without this match, ffmpeg's
                // auto_scaler trips on a -40 ENOSYS reinit when the
                // first segment is a placeholder (yuv420p, untagged)
                // and slot 1 is real Wolf Box footage (yuvj420p, BT.709
                // tagged). One probe per (trip, channel); falls back to
                // Wolf Box-shaped defaults if parsing fails.
                let color_meta = ffmpeg::probe_color_metadata(
                    ffmpeg_path,
                    Path::new(&reference_sibling),
                );

                let scratch_dir = output_root.join(".tmp").join(format!(
                    "{}_{}_{}",
                    item.trip_id,
                    item.tier.as_str(),
                    item.channel.as_str()
                ));
                if let Err(e) = std::fs::create_dir_all(&scratch_dir) {
                    record_failed(
                        db,
                        item,
                        &format!("scratch dir create failed: {e}"),
                    )?;
                    failed += 1;
                    continue;
                }

                let mut built: Vec<String> = Vec::with_capacity(entries.len());
                let mut placeholder_err: Option<AppError> = None;
                for (i, entry) in entries.into_iter().enumerate() {
                    match entry {
                        ConcatEntry::Real(s) => built.push(s),
                        ConcatEntry::MissingPlaceholder { duration_s } => {
                            let path = scratch_dir.join(format!("ph_{i}.mp4"));
                            match ffmpeg::generate_black_placeholder(
                                ffmpeg_path,
                                &path,
                                width,
                                height,
                                fps,
                                duration_s,
                                encoder,
                                &color_meta,
                            ) {
                                Ok(()) => {
                                    built.push(path.to_string_lossy().to_string());
                                    scratch_files.push(path);
                                }
                                Err(e) => {
                                    placeholder_err = Some(e);
                                    break;
                                }
                            }
                        }
                    }
                }
                if let Some(e) = placeholder_err {
                    for f in &scratch_files {
                        let _ = std::fs::remove_file(f);
                    }
                    let _ = std::fs::remove_dir(&scratch_dir);
                    record_failed(
                        db,
                        item,
                        &format!("black placeholder generation failed: {e}"),
                    )?;
                    failed += 1;
                    continue;
                }
                (built, missing_count)
            }
        };

        let output_path = output_root.join(format!(
            "{}_{}_{}.mp4",
            item.trip_id,
            item.tier.as_str(),
            item.channel.as_str()
        ));

        mark_running(db, item)?;

        let args = EncodeArgs {
            ffmpeg_path,
            source_paths: &sources,
            output_path: &output_path,
            tier: item.tier,
            channel: item.channel,
            encoder,
            windows: &ctx.windows,
            total_duration_s: ctx.total_duration_s,
        };
        let encode_result = ffmpeg::encode_trip_channel(&args, cancel);

        // Clean up scratch placeholders before handling the encode
        // result so we never leak temp files, even on cancel.
        for f in &scratch_files {
            let _ = std::fs::remove_file(f);
        }

        match encode_result {
            Ok(path) => {
                // Compute the piecewise speed curve and serialize as
                // JSON so the frontend player can map file-time ↔
                // concat-time during playback. Per (trip, tier); the
                // three channels of the same (trip, tier) get an
                // identical curve because they use the same filter.
                let curve = crate::timelapse::speed_curve::build_curve(
                    &ctx.windows,
                    item.tier,
                    ctx.total_duration_s,
                );
                let curve_json = serde_json::to_string(&curve)
                    .unwrap_or_else(|_| "[]".to_string());
                record_done(
                    db,
                    item,
                    &path,
                    &caps.version,
                    encoder.as_str(),
                    padded_count as i64,
                    &curve_json,
                )?;
                done += 1;
            }
            Err(AppError::Internal(ref msg)) if msg == "cancelled" => {
                // Revert the row so a re-run picks this trip up.
                reset_pending(db, item)?;
                break;
            }
            Err(e) => {
                failed += 1;
                record_failed(db, item, &e.to_string())?;
            }
        }

        if last_emit.elapsed() >= EMIT_INTERVAL {
            let _ = app.emit(
                "timelapse:progress",
                TimelapseProgressEvent {
                    total,
                    done,
                    failed,
                    current_trip_id: Some(item.trip_id.clone()),
                    current_tier: Some(item.tier.as_str().to_string()),
                    current_channel: Some(item.channel.as_str().to_string()),
                },
            );
            last_emit = Instant::now();
        }
    }

    let cancelled = cancel.load(Ordering::Relaxed);
    let _ = app.emit(
        "timelapse:done",
        TimelapseDoneEvent {
            total,
            done,
            failed,
            cancelled,
        },
    );

    Ok(())
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
) -> Result<(), AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::mark_done(
        &conn,
        &item.trip_id,
        item.tier.as_str(),
        item.channel.as_str(),
        &output_path.to_string_lossy(),
        ffmpeg_version,
        encoder_used,
        padded_count,
        speed_curve_json,
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

/// Per-trip state cached across (tier, channel) units. Computed once
/// when the worker transitions to a new trip_id.
struct TripEncodeContext {
    trip_id: String,
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
    let windows = events::detect_events(&stitched);
    Ok(TripEncodeContext {
        trip_id: trip_id.to_string(),
        front_sources,
        front_durations,
        windows,
        total_duration_s,
    })
}

/// One segment's worth of info needed to build the concat list and
/// stitch GPS across the trip.
#[derive(Debug, Clone)]
struct SegmentInfo {
    master_path: String,
    duration_s: f64,
    camera_kind: CameraKind,
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
fn trip_segment_info(db: &DbHandle, trip_id: &str) -> Result<Vec<SegmentInfo>, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let mut stmt = conn.prepare(
        "SELECT master_path, duration_s, camera_kind
         FROM segments WHERE trip_id = ?1 ORDER BY start_time_ms ASC",
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
        let (path, duration, kind) = r?;
        out.push(SegmentInfo {
            master_path: path,
            duration_s: duration,
            camera_kind: camera_kind_from_str(&kind),
        });
    }
    Ok(out)
}

/// Stitch GPS across all segments of a trip, remapping each point's
/// `t_offset_s` to concat-timeline (cumulative duration of prior
/// segments). Segments with no GPS contribute no points but still
/// advance the time cursor. Failed extractions are treated as empty
/// — a missing GPS trace shouldn't fail the whole encode.
fn stitch_trip_gps(segments: &[SegmentInfo]) -> Vec<GpsPoint> {
    let mut out = Vec::new();
    let mut cursor = 0.0;
    for seg in segments {
        let points = gps::extract_for_kind(Path::new(&seg.master_path), seg.camera_kind)
            .unwrap_or_default();
        for p in points {
            out.push(GpsPoint {
                t_offset_s: cursor + p.t_offset_s,
                ..p
            });
        }
        cursor += seg.duration_s;
    }
    out
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

/// Outcome of resolving non-Front sibling paths for a trip. The
/// distinction matters for correctness: a camera that doesn't record
/// this channel at all (single-channel Miltona, Thinkware-without-rear,
/// etc.) should skip silently, but a trip with *some* siblings missing
/// is broken — encoding only the files that exist produces a shorter
/// concat that falls out of sync with the other channels. That's the
/// bug the 14-minute rear drift turned out to be.
/// One entry in the concat list passed to ffmpeg. Either a real
/// on-disk sibling, or a marker that a black placeholder needs to
/// be generated of a given duration.
#[derive(Debug, Clone)]
pub enum ConcatEntry {
    Real(String),
    MissingPlaceholder { duration_s: f64 },
}

#[derive(Debug)]
pub enum SiblingResolution {
    /// Every front segment has a matching sibling on disk.
    Complete(Vec<String>),
    /// No siblings exist for this channel (expected for single-channel
    /// cameras). Caller should skip this (trip, channel) gracefully.
    CameraDoesNotRecord,
    /// Some siblings exist and some don't. Caller generates black
    /// placeholders for the missing positions and proceeds with encode:
    /// output stays the right duration and in sync with sibling
    /// channels, but the affected stretches show black.
    PadWithBlack {
        entries: Vec<ConcatEntry>,
        missing_count: usize,
        /// A real sibling file we can probe (via `mp4_probe`) to get
        /// codec/resolution/framerate for the placeholders so they
        /// match the concat demuxer's uniformity requirement.
        reference_sibling: String,
    },
}

/// Resolve sources for one (trip, channel) pair. Walks the trip's
/// master_paths and probes the requested channel's sibling at each
/// position, classifying the trip as all-present / all-missing /
/// partially-missing. The Front channel goes through the same path
/// as Interior/Rear: when a master happens to be a non-Front file
/// (e.g. the front camera wasn't recording for the first segment of
/// a trip), the Front job correctly gets a placeholder for that slot
/// instead of accidentally concatenating a wrong-channel/wrong-
/// resolution file into the F output.
fn resolve_channel_sources(
    front_sources: &[String],
    front_durations: &[f64],
    channel: Channel,
) -> Result<SiblingResolution, AppError> {
    debug_assert_eq!(front_sources.len(), front_durations.len());

    let mut entries: Vec<ConcatEntry> = Vec::with_capacity(front_sources.len());
    let mut any_present = false;
    let mut missing_count: usize = 0;
    let mut reference: Option<String> = None;

    for (path, duration) in front_sources.iter().zip(front_durations.iter()) {
        match resolve_sibling_path(path, channel)? {
            Some(sibling) => {
                let s = sibling.to_string_lossy().to_string();
                if reference.is_none() {
                    reference = Some(s.clone());
                }
                entries.push(ConcatEntry::Real(s));
                any_present = true;
            }
            None => {
                entries.push(ConcatEntry::MissingPlaceholder { duration_s: *duration });
                missing_count += 1;
            }
        }
    }

    if !any_present {
        return Ok(SiblingResolution::CameraDoesNotRecord);
    }
    if missing_count == 0 {
        let paths = entries
            .into_iter()
            .filter_map(|e| match e {
                ConcatEntry::Real(s) => Some(s),
                ConcatEntry::MissingPlaceholder { .. } => None,
            })
            .collect();
        return Ok(SiblingResolution::Complete(paths));
    }

    Ok(SiblingResolution::PadWithBlack {
        entries,
        missing_count,
        reference_sibling: reference.expect("any_present ensured we have one"),
    })
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
    for trip_id in &trips {
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
    Ok(work)
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

/// Determine `<library_root>/Timelapses/` and create it if needed.
/// Prefers the cached `library_root` setting; falls back to deriving
/// it from any segment's master_path by walking upwards for a
/// directory named "Videos".
fn resolve_output_root(db: &DbHandle) -> Result<PathBuf, AppError> {
    let root = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        db::settings::get(&conn, "library_root")?
    };
    let library_root = match root {
        Some(p) => PathBuf::from(p),
        None => {
            let discovered = discover_library_root(db)?;
            let conn = db
                .lock()
                .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
            db::settings::set(&conn, "library_root", &discovered.to_string_lossy())?;
            discovered
        }
    };
    let out = library_root.join("Timelapses");
    std::fs::create_dir_all(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        match resolve_channel_sources(&sources, &durations, Channel::Front).unwrap() {
            SiblingResolution::Complete(got) => {
                assert_eq!(got.len(), 2);
                assert!(got.iter().any(|s| s.ends_with("103001_00_F.MP4")));
                assert!(got.iter().any(|s| s.ends_with("103300_00_F.MP4")));
            }
            other => panic!("expected Complete, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_front_pads_when_a_master_is_not_front() {
        // The 9:43 AM trip case: the first segment's master is a Rear
        // file (no F existed at that timestamp). The F job for that
        // trip must place a black-frame placeholder for that slot,
        // not concatenate the wrong-channel rear file into the F
        // output (which it used to do via the Front short-circuit and
        // which trivially fails with filter-reinit -40 because the F
        // and R cameras record at different resolutions).
        let dir = unique_dir("front-pads-when-master-is-r");
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
        match resolve_channel_sources(&sources, &durations, Channel::Front).unwrap() {
            SiblingResolution::PadWithBlack {
                entries,
                missing_count,
                reference_sibling,
            } => {
                assert_eq!(missing_count, 1);
                assert_eq!(entries.len(), 2);
                // Slot 0 has no F on disk → placeholder.
                match &entries[0] {
                    ConcatEntry::MissingPlaceholder { duration_s } => {
                        assert!((duration_s - 180.0).abs() < 1e-9);
                    }
                    other => panic!("expected MissingPlaceholder at slot 0, got {other:?}"),
                }
                // Slot 1 has a real F file.
                assert!(matches!(entries[1], ConcatEntry::Real(_)));
                // Reference for placeholder dimensions/fps comes from
                // the real F file, so placeholders match the F camera's
                // resolution rather than the rear camera's.
                assert!(reference_sibling.ends_with("_F.MP4"));
            }
            other => panic!("expected PadWithBlack, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_requests_padding_for_partial_miss() {
        let dir = unique_dir("padmiss");
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
        match resolve_channel_sources(&sources, &durations, Channel::Interior).unwrap() {
            SiblingResolution::PadWithBlack {
                entries,
                missing_count,
                reference_sibling,
            } => {
                assert_eq!(missing_count, 1);
                assert_eq!(entries.len(), 2);
                assert!(reference_sibling.ends_with("_I.MP4"));
                // Position 0 should be real (interior_a exists).
                assert!(matches!(entries[0], ConcatEntry::Real(_)));
                // Position 1 should be a placeholder carrying the
                // front segment's duration (175.5 from durations[1]).
                match &entries[1] {
                    ConcatEntry::MissingPlaceholder { duration_s } => {
                        assert!((duration_s - 175.5).abs() < 1e-9);
                    }
                    other => panic!("expected MissingPlaceholder, got {other:?}"),
                }
            }
            other => panic!("expected PadWithBlack, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_channel_sources_reports_camera_does_not_record_when_all_missing() {
        let dir = unique_dir("allmiss");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let front = dir.join("2026_04_22_103001_00_F.MP4");
        fs::write(&front, b"").unwrap();
        // No rear file at all.

        let sources = vec![front.to_string_lossy().to_string()];
        let durations = vec![180.0];
        match resolve_channel_sources(&sources, &durations, Channel::Rear).unwrap() {
            SiblingResolution::CameraDoesNotRecord => {}
            other => panic!("expected CameraDoesNotRecord, got {other:?}"),
        }

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
        match resolve_channel_sources(&sources, &durations, Channel::Rear).unwrap() {
            SiblingResolution::Complete(v) => {
                assert_eq!(v.len(), 2);
                assert!(v.iter().any(|s| s.ends_with("2026_04_22_103000_00_R.MP4")));
                assert!(v.iter().any(|s| s.ends_with("2026_04_22_103302_00_R.MP4")));
            }
            other => panic!("expected Complete, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }
}

fn discover_library_root(db: &DbHandle) -> Result<PathBuf, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let sample: Option<String> = conn
        .query_row(
            "SELECT master_path FROM segments LIMIT 1",
            [],
            |r| r.get(0),
        )
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
                return Ok(parent.to_path_buf());
            }
        }
    }
    Err(AppError::Internal(format!(
        "could not locate 'Videos' directory in segment path: {}",
        p.display()
    )))
}
