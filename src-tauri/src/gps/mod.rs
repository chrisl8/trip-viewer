//! GPS extraction — dispatches to a brand-specific decoder based on
//! `CameraKind` since each dashcam stores GPS in its own proprietary layout.

pub mod miltona;
pub mod shenshu;
pub mod validation;
pub mod viofo;
pub mod viofo_timing;

use crate::archive::{require_db, ArchiveSlot};
use crate::error::AppError;
use crate::model::{GpsBatchItem, GpsPoint};
use crate::scan::naming::CameraKind;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::State;

/// Bump when GPS decoders change semantics so previously archived GPS becomes
/// stale. The encoder's `has_current` probe and the startup backfill both compare
/// against this; rows below the current version are re-extracted on the next
/// encode (or backfill pass) when the original MP4 is still on disk.
///
/// v2: trip-stitched GPS trims each segment's points to video duration.
/// v3: adds VIOFO / Novatek GPS extraction for A229-family footage and fixes
///     generic VIOFO files being sent through the Wolf Box ShenShu decoder.
/// v4: filters isolated, physically impossible VIOFO position spikes while
///     preserving each file's original GPS time axis.
/// v5: aligns VIOFO UTC GPS timestamps to the clip's filename start time so a
///     delayed first GPS fix remains delayed on the video timeline.
/// v6: validates VIOFO observations in physical source order with the reusable
///     camera-neutral consistency layer before they can become route anchors.
/// v7: gives VIOFO boundary records neighbouring GPS evidence from the adjacent
///     segment when the embedded UTC cadence itself proves local continuity.
/// v8: caches trip GPS for ordinary playback and can choose a healthy sibling
///     camera as the GPS source for each segment instead of assuming Front.
pub const GPS_PARSER_VERSION: i32 = 8;

/// A single path plus the camera brand the scanner identified for it. The
/// frontend builds one of these per segment (by pairing each master channel's
/// file path with its segment's `cameraKind`) and submits them in a batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsRequest {
    pub path: String,
    pub camera_kind: CameraKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripGpsChannelRequest {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripGpsSegmentRequest {
    pub segment_id: String,
    pub duration_s: f64,
    pub camera_kind: CameraKind,
    pub channels: Vec<TripGpsChannelRequest>,
}

#[derive(Debug, Clone)]
pub(crate) struct TripGpsSegmentSource {
    pub duration_s: f64,
    pub camera_kind: CameraKind,
    pub paths: Vec<String>,
}

#[tauri::command]
pub async fn extract_gps(path: String, camera_kind: CameraKind) -> Result<Vec<GpsPoint>, AppError> {
    extract_for_kind(Path::new(&path), camera_kind)
}

/// Internal batch representation. VIOFO keeps its embedded UTC until the whole
/// selected trip has been decoded so a terminal record can be checked against
/// the first trustworthy records of the next segment. Other camera paths remain
/// byte-for-byte on their existing extraction route.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum BatchGps {
    Plain(GpsBatchItem),
    Viofo {
        file_path: String,
        timed: Vec<viofo::TimedGpsPoint>,
    },
}

fn extract_batch_item(path: &str, camera_kind: CameraKind) -> BatchGps {
    let path_obj = Path::new(path);
    if matches!(camera_kind, CameraKind::Generic) && is_viofo_path(path_obj) {
        let timed = extract_viofo_timed(path_obj).unwrap_or_default();
        BatchGps::Viofo {
            file_path: path.to_string(),
            timed,
        }
    } else {
        let points = extract_for_kind(path_obj, camera_kind).unwrap_or_default();
        BatchGps::Plain(GpsBatchItem {
            file_path: path.to_string(),
            points,
        })
    }
}

fn batch_quality(item: &BatchGps, duration_s: f64) -> (usize, i64) {
    let offsets: Vec<f64> = match item {
        BatchGps::Plain(item) => item
            .points
            .iter()
            .filter(|p| p.fix_ok && p.t_offset_s >= 0.0 && p.t_offset_s <= duration_s)
            .map(|p| p.t_offset_s)
            .collect(),
        BatchGps::Viofo { timed, .. } => timed
            .iter()
            .filter(|p| {
                p.point.fix_ok && p.point.t_offset_s >= 0.0 && p.point.t_offset_s <= duration_s
            })
            .map(|p| p.point.t_offset_s)
            .collect(),
    };
    if offsets.is_empty() {
        return (0, 0);
    }
    let span_ms = ((offsets[offsets.len() - 1] - offsets[0]).max(0.0) * 1000.0) as i64;
    (offsets.len(), span_ms)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TripGpsSegmentUpdate {
    pub index: usize,
    pub points: Vec<GpsPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TripGpsProgress {
    Started {
        attempt: usize,
        total: usize,
    },
    Segment {
        attempt: usize,
        completed: usize,
        total: usize,
        updates: Vec<TripGpsSegmentUpdate>,
    },
}

/// Extract and stitch one trip while allowing each segment to pick the camera
/// file with the strongest usable GPS trace. Candidate order is the tie-breaker,
/// so a healthy canonical Front remains preferred when all cameras are equal.
pub(crate) fn extract_best_trip_gps(segments: &[TripGpsSegmentSource]) -> Vec<GpsPoint> {
    match extract_trip_gps_segments::<std::convert::Infallible>(
        segments,
        |_, segment| Ok(select_segment_gps_parallel(segment, extract_batch_item)),
        |_, _| {},
    ) {
        Ok(points) => points,
        Err(never) => match never {},
    }
}

fn trip_segment_points(item: &BatchGps, duration: f64, offset: f64) -> Vec<GpsPoint> {
    let points: Vec<&GpsPoint> = match item {
        BatchGps::Plain(item) => item.points.iter().collect(),
        BatchGps::Viofo { timed, .. } => timed.iter().map(|p| &p.point).collect(),
    };
    points
        .into_iter()
        .filter(|p| p.t_offset_s >= 0.0 && p.t_offset_s <= duration)
        .map(|p| GpsPoint {
            t_offset_s: offset + p.t_offset_s,
            ..p.clone()
        })
        .collect()
}

/// Emit only the current segment and its corrected left neighbour. VIOFO's
/// boundary validation can revise that neighbour when the next file arrives.
/// This preserves the original final result without resending the whole trip.
#[cfg(test)]
fn extract_trip_gps_progressive(
    segments: &[TripGpsSegmentSource],
    mut extract: impl FnMut(&str, CameraKind) -> BatchGps,
    mut progress: impl FnMut(usize, Vec<TripGpsSegmentUpdate>),
) -> Vec<GpsPoint> {
    match extract_trip_gps_segments::<std::convert::Infallible>(
        segments,
        |_, segment| Ok(select_segment_gps(segment, &mut extract)),
        &mut progress,
    ) {
        Ok(points) => points,
        Err(never) => match never {},
    }
}

#[cfg(test)]
fn select_segment_gps(
    segment: &TripGpsSegmentSource,
    mut extract: impl FnMut(&str, CameraKind) -> BatchGps,
) -> BatchGps {
    best_segment_gps(
        segment
            .paths
            .iter()
            .map(|path| extract(path, segment.camera_kind)),
        segment.duration_s,
    )
}

fn best_segment_gps(items: impl IntoIterator<Item = BatchGps>, duration_s: f64) -> BatchGps {
    let mut best: Option<((usize, i64), BatchGps)> = None;
    for item in items {
        let quality = batch_quality(&item, duration_s);
        if best.as_ref().map(|(q, _)| quality > *q).unwrap_or(true) {
            best = Some((quality, item));
        }
    }
    best.map(|(_, item)| item).unwrap_or_else(|| {
        BatchGps::Plain(GpsBatchItem {
            file_path: String::new(),
            points: Vec::new(),
        })
    })
}

fn gps_read_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let workers = std::env::var("TRIPVIEWER_GPS_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(2)
            .clamp(1, 2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|i| format!("gps-reader-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// One shared read budget across trips and batch extraction. Indexed parallel
/// collection preserves candidate order even when Rear finishes before Front.
/// No DB access, record sorting or boundary validation happens on reader jobs.
fn bounded_gps_map<T: Sync, R: Send>(items: &[T], read: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    if let Some(pool) = gps_read_pool() {
        pool.install(|| items.par_iter().map(&read).collect())
    } else {
        // Thread creation failure must neither panic nor lift the I/O bound.
        static FALLBACK: Mutex<()> = Mutex::new(());
        let _guard = FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        items.iter().map(read).collect()
    }
}

fn select_segment_gps_parallel(
    segment: &TripGpsSegmentSource,
    extract: impl Fn(&str, CameraKind) -> BatchGps + Sync + Send,
) -> BatchGps {
    let started = Instant::now();
    let items = bounded_gps_map(&segment.paths, |path| extract(path, segment.camera_kind));
    let result = best_segment_gps(items, segment.duration_s);
    if let Some(path) = segment.paths.first() {
        gps_timing(Path::new(path), "segment_candidates", started.elapsed(), 0);
    }
    result
}

fn gps_timing(path: &Path, phase: &str, elapsed: Duration, records: usize) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var("TRIPVIEWER_GPS_TIMING").as_deref() == Ok("1")) {
        eprintln!(
            "[gps-timing] phase={phase} file={} ms={:.3} records={records}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

fn extract_trip_gps_segments<E>(
    segments: &[TripGpsSegmentSource],
    mut extract: impl FnMut(usize, &TripGpsSegmentSource) -> Result<BatchGps, E>,
    mut progress: impl FnMut(usize, Vec<TripGpsSegmentUpdate>),
) -> Result<Vec<GpsPoint>, E> {
    let mut selected: Vec<BatchGps> = Vec::with_capacity(segments.len());
    let mut offsets = Vec::with_capacity(segments.len());
    let mut cursor = 0.0;
    for (index, segment) in segments.iter().enumerate() {
        offsets.push(cursor);
        cursor += segment.duration_s;
        selected.push(extract(index, segment)?);
        let start = index.saturating_sub(1);
        validate_viofo_batch_boundaries(&mut selected[start..]);
        let updates = (start..=index)
            .map(|i| TripGpsSegmentUpdate {
                index: i,
                points: trip_segment_points(&selected[i], segments[i].duration_s, offsets[i]),
            })
            .collect();
        progress(index + 1, updates);
    }
    Ok(selected
        .iter()
        .enumerate()
        .flat_map(|(i, item)| trip_segment_points(item, segments[i].duration_s, offsets[i]))
        .collect())
}

#[tauri::command]
pub async fn extract_gps_batch(requests: Vec<GpsRequest>) -> Result<Vec<GpsBatchItem>, AppError> {
    let mut extracted: Vec<BatchGps> = bounded_gps_map(&requests, |req| {
        extract_batch_item(&req.path, req.camera_kind)
    });

    validate_viofo_batch_boundaries(&mut extracted);

    Ok(extracted
        .into_iter()
        .map(|item| match item {
            BatchGps::Plain(item) => item,
            BatchGps::Viofo { file_path, timed } => GpsBatchItem {
                file_path,
                points: timed.into_iter().map(|item| item.point).collect(),
            },
        })
        .collect())
}

/// Return cached trip GPS when current; otherwise extract it once from the
/// supplied segment/channel candidates, persist it in trip_gps, and return it.
/// force=true is used after a runtime media failure so a known-bad camera can
/// be excluded for that segment and the persisted track can heal immediately.
#[tauri::command]
pub async fn ensure_trip_gps(
    trip_id: String,
    segments: Vec<TripGpsSegmentRequest>,
    force: Option<bool>,
    on_progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<GpsPoint>, AppError> {
    let db = require_db(&slot)?;
    let on_progress: Option<tauri::ipc::Channel<TripGpsProgress>> =
        on_progress.map(|id| id.channel_on(webview));
    tauri::async_runtime::spawn_blocking(move || {
        let mut attempt = 0;
        ensure_trip_gps_unlocked(&db, &trip_id, &segments, force.unwrap_or(false), |work| {
            attempt += 1;
            let total = work.sources.len();
            if let Some(channel) = &on_progress {
                let _ = channel.send(TripGpsProgress::Started { attempt, total });
            }
            extract_trip_gps_checkpointed_using(
                &db,
                &trip_id,
                &segments,
                work,
                |segment| select_segment_gps_parallel(segment, extract_batch_item),
                |completed, mut updates| {
                    restore_archived_progress(work, &mut updates);
                    if let Some(channel) = &on_progress {
                        let _ = channel.send(TripGpsProgress::Segment {
                            attempt,
                            completed,
                            total,
                            updates,
                        });
                    }
                },
            )
        })
    })
    .await
    .map_err(|e| AppError::Internal(format!("GPS worker failed: {e}")))?
}

struct GpsWork {
    signature: String,
    sources: Vec<TripGpsSegmentSource>,
    available: Vec<bool>,
    cached: Option<Vec<GpsPoint>>,
}

/// Checkpoints contain raw per-segment selections, including embedded UTC.
/// Boundary validation always runs again with the current neighbours. Reads,
/// writes and provenance checks hold the DB mutex only briefly; parsing and
/// JSON serialization/deserialization happen outside it.
#[cfg(test)]
fn extract_trip_gps_checkpointed(
    db: &crate::db::DbHandle,
    trip_id: &str,
    requests: &[TripGpsSegmentRequest],
    work: &GpsWork,
    mut extract: impl FnMut(&str, CameraKind) -> BatchGps,
    progress: impl FnMut(usize, Vec<TripGpsSegmentUpdate>),
) -> Result<Vec<GpsPoint>, AppError> {
    extract_trip_gps_checkpointed_using(
        db,
        trip_id,
        requests,
        work,
        |segment| select_segment_gps(segment, &mut extract),
        progress,
    )
}

fn extract_trip_gps_checkpointed_using(
    db: &crate::db::DbHandle,
    trip_id: &str,
    requests: &[TripGpsSegmentRequest],
    work: &GpsWork,
    mut select: impl FnMut(&TripGpsSegmentSource) -> BatchGps,
    progress: impl FnMut(usize, Vec<TripGpsSegmentUpdate>),
) -> Result<Vec<GpsPoint>, AppError> {
    use crate::db::gps_segment_checkpoints as checkpoints;
    let signatures: Vec<serde_json::Value> = serde_json::from_str(&work.signature)
        .map_err(|e| AppError::Internal(format!("GPS checkpoint signature: {e}")))?;
    extract_trip_gps_segments(
        &work.sources,
        |index, segment| {
            let signature = serde_json::to_string(&vec![&signatures[index]])
                .map_err(|e| AppError::Internal(format!("GPS checkpoint signature: {e}")))?;
            let cached = {
                let conn = db
                    .lock()
                    .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
                checkpoints::load(
                    &conn,
                    trip_id,
                    &requests[index].segment_id,
                    &signature,
                    GPS_PARSER_VERSION,
                )?
            };
            if let Some(payload) = cached {
                match serde_json::from_str::<BatchGps>(&payload) {
                    Ok(mut item) => {
                        let path = batch_path_mut(&mut item);
                        if !path.is_empty() {
                            *path = crate::paths::from_archive_relative(path, db.archive_root())
                                .to_string_lossy()
                                .into_owned();
                        }
                        return Ok(item);
                    }
                    Err(e) => eprintln!("GPS checkpoint invalid; re-extracting segment: {e}"),
                }
            }
            let item = select(segment);
            let mut stored = item.clone();
            let path = batch_path_mut(&mut stored);
            *path = crate::media_health::stored_path(Path::new(path), db.archive_root());
            let payload = serde_json::to_string(&stored)
                .map_err(|e| AppError::Internal(format!("GPS checkpoint serialize: {e}")))?;
            {
                let conn = db
                    .lock()
                    .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
                // A file or health record can change while it is being parsed.
                // Never save that selection under stale provenance.
                if trip_gps_sources(&conn, db.archive_root(), &requests[index..=index])?.0
                    == signature
                {
                    checkpoints::upsert(
                        &conn,
                        trip_id,
                        &requests[index].segment_id,
                        &signature,
                        GPS_PARSER_VERSION,
                        &payload,
                    )?;
                }
            }
            Ok(item)
        },
        progress,
    )
}

fn batch_path_mut(item: &mut BatchGps) -> &mut String {
    match item {
        BatchGps::Plain(item) => &mut item.file_path,
        BatchGps::Viofo { file_path, .. } => file_path,
    }
}

fn restore_archived_progress(work: &GpsWork, updates: &mut [TripGpsSegmentUpdate]) {
    for update in updates {
        if !work.available[update.index] {
            let start: f64 = work.sources[..update.index]
                .iter()
                .map(|s| s.duration_s)
                .sum();
            let end = start + work.sources[update.index].duration_s;
            update.points = work
                .cached
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter(|p| p.t_offset_s >= start && p.t_offset_s < end)
                .cloned()
                .collect();
        }
    }
}

enum GpsPreparation {
    Cached(Vec<GpsPoint>),
    Extract(GpsWork),
}

fn prepare_trip_gps(
    conn: &rusqlite::Connection,
    archive_root: &Path,
    trip_id: &str,
    segments: &[TripGpsSegmentRequest],
    force: bool,
) -> Result<GpsPreparation, AppError> {
    use crate::db::trip_gps;
    let cached = trip_gps::load(conn, trip_id)?;
    let (signature, sources, available) = trip_gps_sources(conn, archive_root, segments)?;
    if !available.iter().any(|exists| *exists) {
        return Ok(GpsPreparation::Cached(cached.unwrap_or_default()));
    }
    if !force
        && trip_gps::has_current(conn, trip_id, GPS_PARSER_VERSION)?
        && trip_gps::source_signature(conn, trip_id)?.as_deref() == Some(&signature)
    {
        return Ok(GpsPreparation::Cached(cached.unwrap_or_default()));
    }
    Ok(GpsPreparation::Extract(GpsWork {
        signature,
        sources,
        available,
        cached,
    }))
}

/// Heavy MP4 I/O must run outside both the async IPC executor and the archive
/// mutex: segment health reads/writes and transport must remain responsive.
fn ensure_trip_gps_unlocked(
    db: &crate::db::DbHandle,
    trip_id: &str,
    segments: &[TripGpsSegmentRequest],
    force: bool,
    mut extract: impl FnMut(&GpsWork) -> Result<Vec<GpsPoint>, AppError>,
) -> Result<Vec<GpsPoint>, AppError> {
    for attempt in 0..3 {
        let preparation = {
            let conn = db
                .lock()
                .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
            prepare_trip_gps(
                &conn,
                db.archive_root(),
                trip_id,
                segments,
                force && attempt == 0,
            )?
        };
        let work = match preparation {
            GpsPreparation::Cached(points) => return Ok(points),
            GpsPreparation::Extract(work) => work,
        };
        let points = extract(&work)?;
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        // Health or fingerprints may change while parsing. Never publish a
        // result under provenance that no longer describes its input files.
        if trip_gps_sources(&conn, db.archive_root(), segments)?.0 != work.signature {
            continue;
        }
        return persist_trip_gps(&conn, trip_id, work, points);
    }
    Err(AppError::Internal(
        "GPS sources changed during preparation; select the trip again to retry".into(),
    ))
}

fn persist_trip_gps(
    conn: &rusqlite::Connection,
    trip_id: &str,
    work: GpsWork,
    mut points: Vec<GpsPoint>,
) -> Result<Vec<GpsPoint>, AppError> {
    use crate::db::trip_gps;
    let GpsWork {
        signature,
        sources,
        available,
        cached,
    } = work;
    // Preserve archived spans in mixed trips too. Empty/no-GPS segments still
    // contribute their duration, so later segments keep their trip time axis.
    if let Some(cached) = cached {
        let mut cursor = 0.0;
        for (index, source) in sources.iter().enumerate() {
            let end = cursor + source.duration_s;
            if !available[index] {
                points.retain(|p| p.t_offset_s < cursor || p.t_offset_s >= end);
                points.extend(
                    cached
                        .iter()
                        .filter(|p| p.t_offset_s >= cursor && p.t_offset_s < end)
                        .cloned(),
                );
            }
            cursor = end;
        }
        points.sort_by(|a, b| a.t_offset_s.total_cmp(&b.t_offset_s));
    }
    // Atomically publish both payload and provenance; a failed statement must
    // not leave a new payload carrying an old signature.
    let tx = conn.unchecked_transaction()?;
    trip_gps::upsert(&tx, trip_id, &points, GPS_PARSER_VERSION)?;
    tx.execute(
        "UPDATE trip_gps SET source_signature = ?2 WHERE trip_id = ?1",
        rusqlite::params![trip_id, signature],
    )?;
    tx.commit()?;
    Ok(points)
}

#[cfg(test)]
fn ensure_trip_gps_cached(
    conn: &rusqlite::Connection,
    root: &Path,
    trip_id: &str,
    segments: &[TripGpsSegmentRequest],
    force: bool,
) -> Result<Vec<GpsPoint>, AppError> {
    match prepare_trip_gps(conn, root, trip_id, segments, force)? {
        GpsPreparation::Cached(points) => Ok(points),
        GpsPreparation::Extract(work) => {
            let points = extract_best_trip_gps(&work.sources);
            persist_trip_gps(conn, trip_id, work, points)
        }
    }
}

fn trip_gps_sources(
    conn: &rusqlite::Connection,
    archive_root: &Path,
    segments: &[TripGpsSegmentRequest],
) -> Result<(String, Vec<TripGpsSegmentSource>, Vec<bool>), AppError> {
    use crate::media_health;
    let mut signature = Vec::new();
    let mut sources = Vec::new();
    let mut available = Vec::new();
    for segment in segments {
        let mut paths = Vec::new();
        let mut has_original = false;
        let mut files = Vec::new();
        for channel in &segment.channels {
            let path = Path::new(&channel.path);
            let stored = media_health::stored_path(path, archive_root);
            let fingerprint = media_health::fingerprint(path).ok();
            let mut ranges = Vec::<(i64, Option<i64>)>::new();
            if let Some((size, modified)) = fingerprint {
                has_original = true;
                let mut stmt = conn.prepare(
                    "SELECT start_ms, end_ms FROM media_health_ranges
                     WHERE file_path = ?1 AND file_size = ?2 AND modified_ms = ?3
                     ORDER BY start_ms, end_ms",
                )?;
                ranges = stmt
                    .query_map(rusqlite::params![stored, size, modified], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?
                    .collect::<Result<_, _>>()?;
                // Even partial video corruption makes a sibling the safer GPS
                // provider for this segment. Never assume GPS implies health.
                if ranges.is_empty() {
                    paths.push(channel.path.clone());
                }
            }
            files.push((stored, fingerprint, ranges));
        }
        signature.push(serde_json::json!([
            segment.segment_id,
            segment.duration_s,
            segment.camera_kind,
            files
        ]));
        available.push(has_original);
        sources.push(TripGpsSegmentSource {
            duration_s: segment.duration_s,
            camera_kind: segment.camera_kind,
            paths,
        });
    }
    let signature = serde_json::to_string(&signature)
        .map_err(|e| AppError::Internal(format!("GPS source signature: {e}")))?;
    Ok((signature, sources, available))
}

/// Load archived trip-stitched GPS from the DB. Returns an empty vec
/// when no row exists for the trip — the frontend treats that as the
/// signal to fall back to the per-segment `extract_gps_batch` path
/// (which only succeeds when originals are still on disk).
#[tauri::command]
pub async fn load_trip_gps(
    trip_id: String,
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<GpsPoint>, AppError> {
    let db = require_db(&slot)?;
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    Ok(crate::db::trip_gps::load(&conn, &trip_id)?.unwrap_or_default())
}

/// Write a diagnostic dump of a Miltona file's `gps0` atom. Used by the
/// "Export GPS debug" UI button to collect ground-truth samples while the
/// lat/lon encoding is still being finalized.
#[tauri::command]
pub async fn dump_miltona_gps_debug(path: String) -> Result<String, AppError> {
    let out = miltona::dump_debug(Path::new(&path))?;
    Ok(out.to_string_lossy().into_owned())
}

fn is_viofo_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(crate::scan::viofo::is_viofo_filename)
        .unwrap_or(false)
}

fn extract_viofo_timed(path: &Path) -> Result<Vec<viofo::TimedGpsPoint>, AppError> {
    // The consistency validator needs the clip's measured duration only as an
    // upper bound on how far apart two UTC records from this same MP4 can be.
    // It does not use the filename clock or infer any timezone.
    let started = Instant::now();
    let duration_s = crate::metadata::mp4_probe::probe(path)?.duration_s;
    gps_timing(path, "metadata", started.elapsed(), 0);
    let mut timed = viofo::extract_timed(path, Some(duration_s))?;
    let Some(first_gps_utc) = timed.first().map(|item| item.utc) else {
        return Ok(timed);
    };

    // Synchronization with the local camera clock remains a separate concern
    // from data validation. For now keep the existing best-effort civil-offset
    // inference, but feed it the first observation that survived consistency
    // validation so malformed raw records cannot become the timing anchor.
    match viofo_timing::first_fix_delay_s(path, first_gps_utc, duration_s) {
        Some(delay_s) if delay_s > 0.0 => {
            for item in &mut timed {
                item.point.t_offset_s += delay_s;
            }
        }
        _ => {}
    }

    Ok(timed)
}

fn extract_viofo(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    Ok(extract_viofo_timed(path)?
        .into_iter()
        .map(|item| item.point)
        .collect())
}

/// Validate only directly adjacent VIOFO batch entries. TripList submits the
/// selected trip's segment masters in segment order. Empty GPS segments are not
/// skipped here: without observations on both sides there is no boundary proof.
fn validate_viofo_batch_boundaries(batch: &mut [BatchGps]) {
    for index in 0..batch.len().saturating_sub(1) {
        let (left_slice, right_slice) = batch.split_at_mut(index + 1);
        let left = &mut left_slice[index];
        let right = &mut right_slice[0];

        let (
            BatchGps::Viofo {
                file_path: left_path,
                timed: left_timed,
            },
            BatchGps::Viofo {
                file_path: right_path,
                timed: right_timed,
            },
        ) = (left, right)
        else {
            continue;
        };

        validate_viofo_boundary(left_path, left_timed, right_path, right_timed);
    }
}

/// A file-end observation has no following neighbour inside its own MP4. The
/// next segment may supply that missing evidence, but only when GPS UTC itself
/// shows that the boundary belongs to the same local sampling sequence.
///
/// No fixed cadence is assumed. The cross-file UTC gap must be no larger than
/// the nearest positive sampling gap observed on either side of the boundary.
/// A delayed next fix or a real recording stop therefore disables the check.
fn validate_viofo_boundary(
    left_path: &str,
    left: &mut Vec<viofo::TimedGpsPoint>,
    right_path: &str,
    right: &mut Vec<viofo::TimedGpsPoint>,
) {
    if left.len() < 2 || right.len() < 2 || !boundary_has_local_utc_continuity(left, right) {
        return;
    }

    let left_last = left.len() - 1;
    let points = [&left[left_last - 1], &left[left_last], &right[0], &right[1]];
    let observations: Vec<validation::GpsObservation> = points
        .iter()
        .enumerate()
        .map(|(source_order, item)| boundary_observation(item, source_order))
        .collect();
    let validated = validation::validate(&observations, validation::ValidationContext::default());

    let drop_left = !validated[1].usable_timed_position();
    let drop_right = !validated[2].usable_timed_position();

    if drop_left {
        left.pop();
        eprintln!(
            "viofo gps: boundary validation excluded terminal route point in {left_path} using UTC evidence from {right_path}"
        );
    }
    if drop_right {
        right.remove(0);
        eprintln!(
            "viofo gps: boundary validation excluded initial route point in {right_path} using UTC evidence from {left_path}"
        );
    }
}

fn boundary_observation(
    item: &viofo::TimedGpsPoint,
    source_order: usize,
) -> validation::GpsObservation {
    // A zero speed is valid data, but after the first validation pass it can
    // also be the neutral UI fallback for an unusable speed/heading field. Do
    // not use such a value as an independent cross-file witness. This only
    // weakens the boundary proof; it never rejects a route point by itself.
    let moving = item.point.speed_mps > 0.0;
    validation::GpsObservation {
        source_order,
        time_ms: Some(item.utc.and_utc().timestamp_millis()),
        latitude: Some(item.point.lat),
        longitude: Some(item.point.lon),
        speed_mps: moving.then_some(item.point.speed_mps),
        heading_deg: moving.then_some(item.point.heading_deg),
    }
}

fn boundary_has_local_utc_continuity(
    left: &[viofo::TimedGpsPoint],
    right: &[viofo::TimedGpsPoint],
) -> bool {
    let left_times: Vec<i64> = left
        .iter()
        .map(|item| item.utc.and_utc().timestamp_millis())
        .collect();
    let right_times: Vec<i64> = right
        .iter()
        .map(|item| item.utc.and_utc().timestamp_millis())
        .collect();
    boundary_times_are_locally_continuous(&left_times, &right_times)
}

fn boundary_times_are_locally_continuous(left: &[i64], right: &[i64]) -> bool {
    if left.len() < 2 || right.len() < 2 {
        return false;
    }

    let cross_gap = right[0] - left[left.len() - 1];
    if cross_gap < 0 {
        return false;
    }

    let left_gap = left
        .windows(2)
        .rev()
        .find_map(|pair| positive_time_gap(pair[0], pair[1]));
    let right_gap = right
        .windows(2)
        .find_map(|pair| positive_time_gap(pair[0], pair[1]));

    match (left_gap, right_gap) {
        (Some(left_gap), Some(right_gap)) => cross_gap <= left_gap.max(right_gap),
        _ => false,
    }
}

fn positive_time_gap(a: i64, b: i64) -> Option<i64> {
    b.checked_sub(a).filter(|gap| *gap > 0)
}

pub fn extract_for_kind(path: &Path, kind: CameraKind) -> Result<Vec<GpsPoint>, AppError> {
    match kind {
        CameraKind::WolfBox => shenshu::extract(path),
        CameraKind::Miltona => miltona::extract(path),
        // Thinkware: no GPS decoder (the sample we have contains no GPS
        // data at all). If a GPS-equipped Thinkware model turns up, add a
        // decoder and flip `CameraKind::gps_supported` for that variant.
        CameraKind::Thinkware => Ok(vec![]),
        // VIOFO is currently represented by the persisted Generic enum value
        // to avoid a DB/frontend migration. Distinguish it by its specific
        // A229-family filename before using the historical generic fallback.
        CameraKind::Generic => {
            if is_viofo_path(path) {
                extract_viofo(path)
            } else {
                shenshu::extract(path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_fixture(root: &Path) -> (crate::db::DbHandle, Vec<TripGpsSegmentRequest>) {
        let db = crate::db::open_in_memory_with_root(root).unwrap();
        db.lock().unwrap().execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
             VALUES ('trip', 0, 60000, 'generic', 1, 0)", []).unwrap();
        let channels = ["front", "interior", "rear"]
            .into_iter()
            .map(|label| {
                let path = root.join(format!("{label}.mp4"));
                std::fs::write(&path, b"not an MP4").unwrap();
                TripGpsChannelRequest {
                    label: label.into(),
                    path: path.to_string_lossy().into(),
                }
            })
            .collect();
        (
            db,
            vec![TripGpsSegmentRequest {
                segment_id: "seg".into(),
                duration_s: 60.0,
                camera_kind: CameraKind::Generic,
                channels,
            }],
        )
    }

    fn seed_cached(conn: &rusqlite::Connection, root: &Path, segments: &[TripGpsSegmentRequest]) {
        let signature = trip_gps_sources(conn, root, segments).unwrap().0;
        crate::db::trip_gps::upsert(
            conn,
            "trip",
            &[timed(1000, 1.0, 1.0, 0.0).point],
            GPS_PARSER_VERSION,
        )
        .unwrap();
        conn.execute(
            "UPDATE trip_gps SET source_signature = ?1 WHERE trip_id = 'trip'",
            [signature],
        )
        .unwrap();
    }

    #[test]
    fn health_reads_and_writes_continue_during_gps_extraction_and_stale_result_is_retried() {
        use std::sync::mpsc;
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let (db, segments) = cache_fixture(root.path());
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker_db = db.clone();
        let worker_segments = segments.clone();
        let worker = std::thread::spawn(move || {
            let mut calls = 0;
            let result =
                ensure_trip_gps_unlocked(&worker_db, "trip", &worker_segments, false, |work| {
                    let sources = &work.sources;
                    calls += 1;
                    if calls == 1 {
                        assert_eq!(sources[0].paths.len(), 3);
                        started_tx.send(()).unwrap();
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    } else {
                        assert_eq!(sources[0].paths.len(), 2);
                    }
                    Ok(vec![timed(1000, calls as f64, 1.0, 0.0).point])
                })
                .unwrap();
            (calls, result)
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (health_tx, health_rx) = mpsc::channel();
        let health_db = db.clone();
        let front = segments[0].channels[0].path.clone();
        let health = std::thread::spawn(move || {
            let conn = health_db.lock().unwrap();
            let path = Path::new(&front);
            let (size, modified) = crate::media_health::fingerprint(path).unwrap();
            conn.execute("INSERT INTO media_health_ranges (file_path, file_size, modified_ms, start_ms, error_kind, updated_at_ms)
                VALUES (?1, ?2, ?3, 1000, 'decode', 0)",
                rusqlite::params![crate::media_health::stored_path(path, health_db.archive_root()), size, modified]).unwrap();
            health_tx.send(()).unwrap();
        });
        let responsive = health_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        resume_tx.send(()).unwrap();
        let (calls, result) = worker.join().unwrap();
        health.join().unwrap();
        assert!(responsive, "GPS parsing must not hold the archive DB mutex");
        assert_eq!(calls, 2, "health changed while parsing; discard stale GPS");
        assert_eq!(result[0].lon, 2.0);
        let conn = db.lock().unwrap();
        assert_eq!(
            crate::db::trip_gps::load(&conn, "trip").unwrap().unwrap()[0].lon,
            2.0
        );
        assert_eq!(
            crate::db::trip_gps::source_signature(&conn, "trip")
                .unwrap()
                .unwrap(),
            trip_gps_sources(&conn, root.path(), &segments).unwrap().0
        );
    }

    #[test]
    fn current_cache_survives_restart_without_reading_mp4_contents() {
        let root = tempfile::tempdir().unwrap();
        let (db, segments) = cache_fixture(root.path());
        let conn = db.lock().unwrap();
        seed_cached(&conn, root.path(), &segments);
        // The files contain invalid MP4 bytes. Re-extraction would return empty.
        assert_eq!(
            ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, false)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn gps_cache_provenance_survives_database_reopen_and_preserves_archived_spans() {
        let root = tempfile::tempdir().unwrap();
        let (_, mut segments) = cache_fixture(root.path());
        let path = root.path().join("cache.sqlite");
        {
            let db = crate::db::open_at_path(&path, root.path()).unwrap();
            let conn = db.lock().unwrap();
            conn.execute("INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
                VALUES ('trip', 0, 120000, 'generic', 1, 0)", []).unwrap();
            seed_cached(&conn, root.path(), &segments);
        }
        let db = crate::db::open_at_path(&path, root.path()).unwrap();
        let conn = db.lock().unwrap();
        assert_eq!(
            ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, false)
                .unwrap()
                .len(),
            1
        );
        // First segment has since been archived. Its cached point must survive
        // rebuilding a later original segment, with the original trip offset.
        segments[0].channels.clear();
        segments.push(TripGpsSegmentRequest {
            segment_id: "later".into(),
            duration_s: 60.0,
            camera_kind: CameraKind::Generic,
            channels: vec![TripGpsChannelRequest {
                label: "Rear".into(),
                path: root.path().join("rear.mp4").to_string_lossy().into(),
            }],
        });
        let points = ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, false).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].t_offset_s, 1.0);
        assert_eq!(
            crate::db::trip_gps::load(&conn, "trip")
                .unwrap()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn size_mtime_parser_and_health_each_invalidate_gps_cache() {
        let root = tempfile::tempdir().unwrap();
        let (db, segments) = cache_fixture(root.path());
        let conn = db.lock().unwrap();
        for change in 0..4 {
            seed_cached(&conn, root.path(), &segments);
            match change {
                0 => std::fs::write(
                    &segments[0].channels[0].path,
                    b"replacement with different size",
                )
                .unwrap(),
                1 => {
                    std::fs::File::options()
                        .write(true)
                        .open(&segments[0].channels[0].path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1234),
                        ))
                        .unwrap()
                }
                2 => {
                    conn.execute("UPDATE trip_gps SET parser_version = 1", [])
                        .unwrap();
                }
                _ => {
                    let path = Path::new(&segments[0].channels[0].path);
                    let (size, modified) = crate::media_health::fingerprint(path).unwrap();
                    conn.execute("INSERT INTO media_health_ranges (file_path, file_size, modified_ms, start_ms, error_kind, updated_at_ms)
                        VALUES (?1, ?2, ?3, 1000, 'decode', 0)",
                        rusqlite::params![crate::media_health::stored_path(path, root.path()), size, modified]).unwrap();
                }
            }
            assert!(
                ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, false)
                    .unwrap()
                    .is_empty(),
                "change {change}"
            );
        }
    }

    #[test]
    fn partial_health_failure_excludes_only_matching_camera_fingerprint() {
        let root = tempfile::tempdir().unwrap();
        let (db, segments) = cache_fixture(root.path());
        let conn = db.lock().unwrap();
        let path = Path::new(&segments[0].channels[0].path);
        let (size, modified) = crate::media_health::fingerprint(path).unwrap();
        conn.execute("INSERT INTO media_health_ranges (file_path, file_size, modified_ms, start_ms, end_ms, error_kind, updated_at_ms)
            VALUES (?1, ?2, ?3, 1000, 2000, 'decode', 0)",
            rusqlite::params![crate::media_health::stored_path(path, root.path()), size, modified]).unwrap();
        let (_, sources, _) = trip_gps_sources(&conn, root.path(), &segments).unwrap();
        assert_eq!(
            sources[0].paths,
            segments[0].channels[1..]
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>()
        );
        std::fs::write(path, b"repaired replacement").unwrap();
        assert_eq!(
            trip_gps_sources(&conn, root.path(), &segments).unwrap().1[0]
                .paths
                .len(),
            3
        );
    }

    #[test]
    fn archive_only_keeps_gps_even_with_old_parser_and_force_refresh() {
        let root = tempfile::tempdir().unwrap();
        let (db, segments) = cache_fixture(root.path());
        let conn = db.lock().unwrap();
        seed_cached(&conn, root.path(), &segments);
        conn.execute("UPDATE trip_gps SET parser_version = 1", [])
            .unwrap();
        for channel in &segments[0].channels {
            std::fs::remove_file(&channel.path).unwrap();
        }
        assert_eq!(
            ensure_trip_gps_cached(&conn, root.path(), "trip", &segments, true)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ensure_trip_gps_cached(&conn, root.path(), "trip", &[], true)
                .unwrap()
                .len(),
            1
        );
    }

    fn timed(ms: i64, lon: f64, speed_mps: f64, heading_deg: f64) -> viofo::TimedGpsPoint {
        let utc = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
            .expect("valid test timestamp")
            .naive_utc();
        viofo::TimedGpsPoint {
            utc,
            point: GpsPoint {
                t_offset_s: ms as f64 / 1000.0,
                lat: 0.0,
                lon,
                speed_mps,
                heading_deg,
                altitude_m: 0.0,
                fix_ok: true,
            },
        }
    }

    fn checkpoint_fixture(root: &Path) -> (crate::db::DbHandle, Vec<TripGpsSegmentRequest>) {
        let (fixture, _) = cache_fixture(root);
        drop(fixture);
        let db = crate::db::open(root).unwrap();
        // Production scan paths share the canonical archive root. Temp paths
        // may be aliases (macOS /var vs /private/var, Windows short names), so
        // exercise the same contract instead of mixing two spellings.
        let root = db.archive_root();
        db.lock().unwrap().execute(
            "INSERT INTO trips (id,start_time_ms,end_time_ms,camera_kind,gps_supported,last_seen_ms)
             VALUES ('trip',0,30000,'generic',1,0)", []).unwrap();
        let requests = (0..3)
            .map(|index| TripGpsSegmentRequest {
                segment_id: format!("seg-{index}"),
                duration_s: 10.0,
                camera_kind: CameraKind::Generic,
                channels: if index == 1 {
                    vec![]
                } else {
                    vec![TripGpsChannelRequest {
                        label: "Front".into(),
                        path: root
                            .join(if index == 0 { "front.mp4" } else { "rear.mp4" })
                            .to_string_lossy()
                            .into(),
                    }]
                },
            })
            .collect();
        (db, requests)
    }

    fn pending_work(db: &crate::db::DbHandle, requests: &[TripGpsSegmentRequest]) -> GpsWork {
        match prepare_trip_gps(
            &db.lock().unwrap(),
            db.archive_root(),
            "trip",
            requests,
            true,
        )
        .unwrap()
        {
            GpsPreparation::Extract(work) => work,
            _ => panic!("expected extraction"),
        }
    }

    fn fake_batch(path: &str, lon: f64) -> BatchGps {
        BatchGps::Plain(GpsBatchItem {
            file_path: path.into(),
            points: vec![timed(1_000, lon, 1.0, 0.0).point],
        })
    }

    #[test]
    fn checkpoints_resume_after_db_reopen_without_reparsing_completed_or_gpsless_segments() {
        let root = tempfile::tempdir().unwrap();
        let (db, requests) = checkpoint_fixture(root.path());
        let mut work = pending_work(&db, &requests);
        // Simulate exiting after the first two segments, before publishing the
        // complete trip. Checkpoints must survive independently of trip_gps.
        work.sources.truncate(2);
        extract_trip_gps_checkpointed(
            &db,
            "trip",
            &requests,
            &work,
            |path, _| fake_batch(path, 1.0),
            |_, _| {},
        )
        .unwrap();
        assert!(crate::db::trip_gps::load(&db.lock().unwrap(), "trip")
            .unwrap()
            .is_none());
        let stored: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload_json FROM gps_segment_checkpoints WHERE segment_id='seg-0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            matches!(serde_json::from_str::<BatchGps>(&stored).unwrap(),
                BatchGps::Plain(item) if item.file_path == "front.mp4"),
            "stored paths must be archive-relative on every platform"
        );
        drop(db);
        let db = crate::db::open(root.path()).unwrap();
        let completed = std::cell::Cell::new(0);
        let mut parsed = Vec::new();
        let result = ensure_trip_gps_unlocked(&db, "trip", &requests, false, |work| {
            extract_trip_gps_checkpointed(
                &db,
                "trip",
                &requests,
                work,
                |path, _| {
                    assert_eq!(
                        completed.get(),
                        2,
                        "restore previews before reading the unfinished segment"
                    );
                    parsed.push(path.to_string());
                    fake_batch(path, 2.0)
                },
                |count, _| completed.set(count),
            )
        })
        .unwrap();
        assert_eq!(parsed, vec![requests[2].channels[0].path.clone()]);
        assert_eq!(
            result.iter().map(|p| p.t_offset_s).collect::<Vec<_>>(),
            vec![1.0, 21.0]
        );
        assert_eq!(completed.get(), 3);
        assert_eq!(
            crate::db::trip_gps::load(&db.lock().unwrap(), "trip")
                .unwrap()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn checkpoints_invalidate_only_changed_sources_parser_or_health() {
        let root = tempfile::tempdir().unwrap();
        let (db, requests) = checkpoint_fixture(root.path());
        let run = |parsed: &mut Vec<String>| {
            let work = pending_work(&db, &requests);
            extract_trip_gps_checkpointed(
                &db,
                "trip",
                &requests,
                &work,
                |path, _| {
                    parsed.push(path.to_string());
                    fake_batch(path, 1.0)
                },
                |_, _| {},
            )
            .unwrap()
        };
        let mut parsed = Vec::new();
        run(&mut parsed);
        assert_eq!(parsed.len(), 2);
        parsed.clear();
        run(&mut parsed);
        assert!(parsed.is_empty());
        std::fs::write(&requests[0].channels[0].path, b"changed size").unwrap();
        run(&mut parsed);
        assert_eq!(parsed, vec![requests[0].channels[0].path.clone()]);
        parsed.clear();
        db.lock()
            .unwrap()
            .execute(
                "UPDATE gps_segment_checkpoints SET parser_version=-1 WHERE segment_id='seg-2'",
                [],
            )
            .unwrap();
        run(&mut parsed);
        assert_eq!(parsed, vec![requests[2].channels[0].path.clone()]);
        parsed.clear();
        let path = Path::new(&requests[0].channels[0].path);
        let (size, modified) = crate::media_health::fingerprint(path).unwrap();
        db.lock().unwrap().execute(
            "INSERT INTO media_health_ranges(file_path,file_size,modified_ms,start_ms,error_kind,updated_at_ms)
             VALUES (?1,?2,?3,0,'decode',0)", rusqlite::params![crate::media_health::stored_path(path, db.archive_root()),size,modified]).unwrap();
        let result = run(&mut parsed);
        assert!(
            parsed.is_empty(),
            "unaffected segment is reused; failed source is not parsed"
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].t_offset_s, 21.0);
    }

    #[test]
    fn checkpoints_reject_health_changes_during_parsing_without_holding_the_db_lock() {
        let root = tempfile::tempdir().unwrap();
        let (db, mut requests) = checkpoint_fixture(root.path());
        requests.truncate(1);
        let mut parsed = 0;
        let result = ensure_trip_gps_unlocked(&db, "trip", &requests, false, |work| {
            extract_trip_gps_checkpointed(&db, "trip", &requests, work, |path, _| {
                parsed += 1;
                let conn = db.lock().unwrap();
                let path_obj = Path::new(path);
                let (size, modified) = crate::media_health::fingerprint(path_obj).unwrap();
                conn.execute(
                    "INSERT INTO media_health_ranges(file_path,file_size,modified_ms,start_ms,error_kind,updated_at_ms)
                     VALUES (?1,?2,?3,0,'decode',0)", rusqlite::params![crate::media_health::stored_path(path_obj, db.archive_root()),size,modified]).unwrap();
                fake_batch(path, 1.0)
            }, |_, _| {})
        }).unwrap();
        assert_eq!(parsed, 1);
        assert!(
            result.is_empty(),
            "discard the GPS from the newly failed camera"
        );
        let stored: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload_json FROM gps_segment_checkpoints",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            matches!(serde_json::from_str::<BatchGps>(&stored).unwrap(), BatchGps::Plain(item) if item.points.is_empty())
        );
    }

    #[test]
    fn checkpoints_revalidate_raw_viofo_boundaries_when_the_neighbour_changes() {
        let root = tempfile::tempdir().unwrap();
        let (db, mut requests) = checkpoint_fixture(root.path());
        requests.remove(1);
        for request in &mut requests {
            request.duration_s = 3.0;
        }
        let run = |pause: bool, parsed: &mut usize| {
            let work = pending_work(&db, &requests);
            extract_trip_gps_checkpointed(
                &db,
                "trip",
                &requests,
                &work,
                |path, _| {
                    *parsed += 1;
                    let left = path == requests[0].channels[0].path;
                    let timed = if left {
                        vec![
                            timed(0, 0.0, 11.12, 90.0),
                            timed(1_000, 0.0001, 11.12, 90.0),
                            timed(2_000, 20.0, 0.0, 0.0),
                        ]
                    } else {
                        let base = if pause { 60_000 } else { 3_000 };
                        let mut points = vec![
                            timed(base, 0.0003, 11.12, 90.0),
                            timed(base + 1_000, 0.0004, 11.12, 90.0),
                        ];
                        for p in &mut points {
                            p.point.t_offset_s -= base as f64 / 1000.0;
                        }
                        points
                    };
                    BatchGps::Viofo {
                        file_path: path.into(),
                        timed,
                    }
                },
                |_, _| {},
            )
            .unwrap()
        };
        let mut parsed = 0;
        assert_eq!(
            run(false, &mut parsed).len(),
            4,
            "remove corrupt terminal point at continuous boundary"
        );
        assert_eq!(parsed, 2);
        std::fs::write(&requests[1].channels[0].path, b"changed neighbour").unwrap();
        parsed = 0;
        assert_eq!(
            run(true, &mut parsed).len(),
            5,
            "raw left segment must be revalidated against changed neighbour"
        );
        assert_eq!(parsed, 1, "only changed neighbour needs reparsing");
    }

    #[test]
    fn parallel_candidates_keep_canonical_ties_despite_reversed_completion() {
        let source = TripGpsSegmentSource {
            duration_s: 10.0,
            camera_kind: CameraKind::Generic,
            paths: vec!["front".into(), "interior".into(), "rear".into()],
        };
        let decode = |path: &str, _: CameraKind| {
            if path == "front" {
                std::thread::sleep(Duration::from_millis(15));
            }
            fake_batch(path, if path == "front" { 1.0 } else { 2.0 })
        };
        let sequential = select_segment_gps(&source, decode);
        let parallel = select_segment_gps_parallel(&source, decode);
        assert_eq!(
            serde_json::to_string(&parallel).unwrap(),
            serde_json::to_string(&sequential).unwrap()
        );
        assert!(matches!(parallel, BatchGps::Plain(item) if item.file_path == "front"));
    }

    #[test]
    fn all_jobs_share_at_most_two_gps_readers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let decode = |_: &usize| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            active.fetch_sub(1, Ordering::SeqCst);
            now
        };
        std::thread::scope(|scope| {
            let a = scope.spawn(|| bounded_gps_map(&[0, 1, 2], &decode));
            let b = scope.spawn(|| bounded_gps_map(&[0, 1, 2], &decode));
            assert_eq!(a.join().unwrap().len(), 3);
            assert_eq!(b.join().unwrap().len(), 3);
        });
        assert!(peak.load(Ordering::SeqCst) <= 2);
        assert!(peak.load(Ordering::SeqCst) >= 1);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn parallel_stitch_matches_sequential_viofo_boundaries_gaps_and_empty_candidates() {
        let sources: Vec<_> = [vec!["left", "empty"], vec!["right"], vec![], vec!["last"]]
            .into_iter()
            .map(|paths| TripGpsSegmentSource {
                duration_s: 3.0,
                camera_kind: CameraKind::Generic,
                paths: paths.into_iter().map(String::from).collect(),
            })
            .collect();
        let decode = |path: &str, _: CameraKind| {
            if path == "empty" {
                return BatchGps::Plain(GpsBatchItem {
                    file_path: path.into(),
                    points: vec![],
                });
            }
            let timed = if path == "left" {
                vec![
                    timed(0, 0.0, 11.12, 90.0),
                    timed(1_000, 0.0001, 11.12, 90.0),
                    timed(2_000, 20.0, 0.0, 0.0),
                ]
            } else {
                let base = if path == "right" { 3_000 } else { 60_000 };
                let mut points = vec![
                    timed(base, 0.0003, 11.12, 90.0),
                    timed(base + 1_000, 0.0004, 11.12, 90.0),
                ];
                for p in &mut points {
                    p.point.t_offset_s -= base as f64 / 1000.0;
                }
                points
            };
            BatchGps::Viofo {
                file_path: path.into(),
                timed,
            }
        };
        let seq = extract_trip_gps_progressive(&sources, decode, |_, _| {});
        let par = extract_trip_gps_segments::<std::convert::Infallible>(
            &sources,
            |_, source| Ok(select_segment_gps_parallel(source, decode)),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&seq).unwrap(),
            serde_json::to_string(&par).unwrap()
        );
        assert_eq!(par.len(), 6);
        assert_eq!(par.last().unwrap().t_offset_s, 10.0);
    }

    #[test]
    fn checkpoint_hit_does_not_submit_candidate_read_jobs() {
        let root = tempfile::tempdir().unwrap();
        let (db, requests) = checkpoint_fixture(root.path());
        let work = pending_work(&db, &requests);
        extract_trip_gps_checkpointed_using(
            &db,
            "trip",
            &requests,
            &work,
            |source| select_segment_gps_parallel(source, |path, _| fake_batch(path, 1.0)),
            |_, _| {},
        )
        .unwrap();
        let result = extract_trip_gps_checkpointed_using(
            &db,
            "trip",
            &requests,
            &work,
            |_| panic!("current checkpoint must bypass all readers"),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn progressive_extraction_emits_before_next_segment_and_keeps_gap_offsets() {
        let completed = std::cell::Cell::new(0);
        let segments: Vec<_> = [vec!["weak", "strong"], vec![], vec!["last"]]
            .into_iter()
            .map(|paths| TripGpsSegmentSource {
                duration_s: 10.0,
                camera_kind: CameraKind::Generic,
                paths: paths.into_iter().map(String::from).collect(),
            })
            .collect();
        let points = extract_trip_gps_progressive(
            &segments,
            |path, _| {
                if path == "last" {
                    assert_eq!(completed.get(), 2);
                }
                let mut points = vec![timed(1_000, 0.0, 0.0, 0.0).point];
                if path == "strong" {
                    points.push(timed(2_000, 0.0, 0.0, 0.0).point);
                }
                BatchGps::Plain(GpsBatchItem {
                    file_path: path.into(),
                    points,
                })
            },
            |count, updates| {
                if count == 1 {
                    assert_eq!(updates[0].points.len(), 2);
                }
                if count == 2 {
                    assert!(updates[1].points.is_empty());
                }
                completed.set(count);
            },
        );
        assert_eq!(completed.get(), 3);
        assert_eq!(
            points.iter().map(|p| p.t_offset_s).collect::<Vec<_>>(),
            vec![1.0, 2.0, 21.0]
        );
    }

    #[test]
    fn archived_progress_keeps_cached_span_without_overriding_available_segment() {
        let cached = vec![
            timed(1_000, 0.0, 0.0, 0.0).point,
            timed(11_000, 0.0, 0.0, 0.0).point,
        ];
        let work = GpsWork {
            signature: String::new(),
            sources: (0..2)
                .map(|_| TripGpsSegmentSource {
                    duration_s: 10.0,
                    camera_kind: CameraKind::Generic,
                    paths: vec![],
                })
                .collect(),
            available: vec![false, true],
            cached: Some(cached),
        };
        let mut updates = vec![
            TripGpsSegmentUpdate {
                index: 0,
                points: vec![],
            },
            TripGpsSegmentUpdate {
                index: 1,
                points: vec![],
            },
        ];
        restore_archived_progress(&work, &mut updates);
        assert_eq!(updates[0].points.len(), 1);
        assert_eq!(updates[0].points[0].t_offset_s, 1.0);
        assert!(updates[1].points.is_empty());
    }

    #[test]
    fn progressive_boundary_correction_replaces_previous_preview() {
        let segments: Vec<_> = ["left", "right"]
            .into_iter()
            .map(|path| TripGpsSegmentSource {
                duration_s: 3.0,
                camera_kind: CameraKind::Generic,
                paths: vec![path.into()],
            })
            .collect();
        let mut previews = std::collections::BTreeMap::new();
        let result = extract_trip_gps_progressive(
            &segments,
            |path, _| {
                let mut timed = if path == "left" {
                    vec![
                        timed(0, 0.0, 11.12, 90.0),
                        timed(1_000, 0.0001, 11.12, 90.0),
                        timed(2_000, 20.0, 0.0, 0.0),
                    ]
                } else {
                    vec![
                        timed(3_000, 0.0003, 11.12, 90.0),
                        timed(4_000, 0.0004, 11.12, 90.0),
                    ]
                };
                if path == "right" {
                    for p in &mut timed {
                        p.point.t_offset_s -= 3.0;
                    }
                }
                BatchGps::Viofo {
                    file_path: path.into(),
                    timed,
                }
            },
            |count, updates| {
                if count == 1 {
                    assert_eq!(updates[0].points.len(), 3);
                }
                if count == 2 {
                    assert_eq!(updates[0].index, 0);
                    assert_eq!(updates[0].points.len(), 2);
                }
                for update in updates {
                    previews.insert(update.index, update.points);
                }
            },
        );
        let preview: Vec<_> = previews
            .into_values()
            .flatten()
            .map(|p| (p.t_offset_s, p.lon))
            .collect();
        assert_eq!(
            preview,
            result
                .into_iter()
                .map(|p| (p.t_offset_s, p.lon))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn boundary_cadence_is_derived_from_observed_utc() {
        assert!(boundary_times_are_locally_continuous(
            &[0, 2_000, 4_000],
            &[6_000, 8_000, 10_000]
        ));
        assert!(boundary_times_are_locally_continuous(
            &[0, 1_000, 1_000],
            &[2_000, 2_000, 3_000]
        ));
        assert!(!boundary_times_are_locally_continuous(
            &[0, 1_000, 2_000],
            &[60_000, 61_000]
        ));
    }

    #[test]
    fn boundary_context_rejects_corrupt_terminal_position() {
        let mut left = vec![
            timed(0, 0.0, 11.12, 90.0),
            timed(1_000, 0.0001, 11.12, 90.0),
            timed(2_000, 20.0, 0.0, 0.0),
        ];
        let mut right = vec![
            timed(3_000, 0.0003, 11.12, 90.0),
            timed(4_000, 0.0004, 11.12, 90.0),
        ];

        validate_viofo_boundary("left", &mut left, "right", &mut right);

        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        assert_eq!(left.last().unwrap().point.lon, 0.0001);
    }

    #[test]
    fn boundary_context_does_not_bridge_a_real_utc_pause() {
        let mut left = vec![
            timed(0, 0.0, 11.12, 90.0),
            timed(1_000, 0.0001, 11.12, 90.0),
            timed(2_000, 20.0, 0.0, 0.0),
        ];
        let mut right = vec![
            timed(60_000, 0.0003, 11.12, 90.0),
            timed(61_000, 0.0004, 11.12, 90.0),
        ];

        validate_viofo_boundary("left", &mut left, "right", &mut right);

        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 2);
    }
}
