use std::collections::HashMap;

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::error::AppError;
use crate::model::{Segment, Trip};
use crate::scan::naming::CameraKind;

/// Thin DB-row view of a segment. Carries exactly the fields the scan
/// worker needs to hand to each `Scan::run`, without the channel list or
/// other frontend-facing data.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by Tasks 7-9 scan implementations
pub struct SegmentRecord {
    pub id: String,
    pub trip_id: String,
    pub master_path: String,
    pub is_event: bool,
    pub camera_kind: CameraKind,
    pub gps_supported: bool,
    pub duration_s: f64,
}

fn camera_kind_from_str(s: &str) -> CameraKind {
    match s {
        "wolfBox" => CameraKind::WolfBox,
        "thinkware" => CameraKind::Thinkware,
        "miltona" => CameraKind::Miltona,
        _ => CameraKind::Generic,
    }
}

fn camera_kind_to_str(kind: CameraKind) -> &'static str {
    match kind {
        CameraKind::WolfBox => "wolfBox",
        CameraKind::Thinkware => "thinkware",
        CameraKind::Miltona => "miltona",
        CameraKind::Generic => "generic",
    }
}

pub fn all_segments(conn: &Connection) -> Result<Vec<SegmentRecord>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, trip_id, master_path, is_event, camera_kind, gps_supported, duration_s
         FROM segments ORDER BY start_time_ms",
    )?;
    let rows = stmt.query_map([], |r| {
        let kind_str: String = r.get("camera_kind")?;
        Ok(SegmentRecord {
            id: r.get("id")?,
            trip_id: r.get("trip_id")?,
            master_path: r.get("master_path")?,
            is_event: r.get::<_, i64>("is_event")? != 0,
            camera_kind: camera_kind_from_str(&kind_str),
            gps_supported: r.get::<_, i64>("gps_supported")? != 0,
            duration_s: r.get("duration_s")?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn upsert_segment(conn: &Connection, seg: &Segment, trip_id: &str, now_ms: i64) -> Result<(), AppError> {
    let master_path = seg
        .channels
        .first()
        .map(|c| c.file_path.as_str())
        .unwrap_or("");
    // Don't overwrite a previously-known size_bytes with NULL when
    // the current scan failed to stat the files — keeps the
    // library-wide totals stable across transient stat hiccups.
    conn.execute(
        "INSERT INTO segments (id, trip_id, start_time_ms, duration_s, master_path, is_event, camera_kind, gps_supported, last_seen_ms, size_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            trip_id = excluded.trip_id,
            start_time_ms = excluded.start_time_ms,
            duration_s = excluded.duration_s,
            master_path = excluded.master_path,
            is_event = excluded.is_event,
            camera_kind = excluded.camera_kind,
            gps_supported = excluded.gps_supported,
            last_seen_ms = excluded.last_seen_ms,
            size_bytes = COALESCE(excluded.size_bytes, segments.size_bytes)",
        params![
            seg.id.to_string(),
            trip_id,
            seg.start_time.and_utc().timestamp_millis(),
            seg.duration_s,
            master_path,
            seg.is_event as i32,
            camera_kind_to_str(seg.camera_kind),
            seg.gps_supported as i32,
            now_ms,
            seg.size_bytes.map(|n| n as i64),
        ],
    )?;
    Ok(())
}

pub fn upsert_trip(conn: &Connection, trip: &Trip, now_ms: i64) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
            start_time_ms = excluded.start_time_ms,
            end_time_ms = excluded.end_time_ms,
            camera_kind = excluded.camera_kind,
            gps_supported = excluded.gps_supported,
            last_seen_ms = excluded.last_seen_ms",
        params![
            trip.id.to_string(),
            trip.start_time.and_utc().timestamp_millis(),
            trip.end_time.and_utc().timestamp_millis(),
            camera_kind_to_str(trip.camera_kind),
            trip.gps_supported as i32,
            now_ms,
        ],
    )?;
    Ok(())
}

/// Load every trip from the DB that has at least one timelapse_jobs row
/// but currently has no segments. These are the archive-only trips —
/// the source MP4s have been deleted but the trip's pre-rendered
/// timelapse(s) remain. Returned with `archive_only = true` and an empty
/// `segments` vec so the frontend can interleave them with the scanned
/// trip list and still drive playback through the tier path.
pub fn list_archive_only_trips(conn: &Connection) -> Result<Vec<Trip>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.start_time_ms, t.end_time_ms, t.camera_kind, t.gps_supported
         FROM trips t
         WHERE EXISTS (SELECT 1 FROM timelapse_jobs j WHERE j.trip_id = t.id)
           AND NOT EXISTS (SELECT 1 FROM segments s WHERE s.trip_id = t.id)
         ORDER BY t.start_time_ms",
    )?;
    let rows = stmt.query_map([], |r| {
        let id_str: String = r.get("id")?;
        let start_ms: i64 = r.get("start_time_ms")?;
        let end_ms: i64 = r.get("end_time_ms")?;
        let kind_str: String = r.get("camera_kind")?;
        let gps_supported: i64 = r.get("gps_supported")?;
        Ok((id_str, start_ms, end_ms, kind_str, gps_supported))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id_str, start_ms, end_ms, kind_str, gps_supported) = row?;
        let id = uuid::Uuid::parse_str(&id_str)
            .map_err(|e| AppError::Internal(format!("invalid trip uuid {id_str}: {e}")))?;
        let start_time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start_ms)
            .ok_or_else(|| AppError::Internal(format!("invalid start_time_ms {start_ms}")))?
            .naive_utc();
        let end_time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(end_ms)
            .ok_or_else(|| AppError::Internal(format!("invalid end_time_ms {end_ms}")))?
            .naive_utc();
        out.push(Trip {
            id,
            start_time,
            end_time,
            segments: Vec::new(),
            camera_kind: camera_kind_from_str(&kind_str),
            gps_supported: gps_supported != 0,
            archive_only: true,
        });
    }
    Ok(out)
}

/// Upsert all trips and their segments in a single transaction, then
/// delete any segment/trip rows whose `last_seen_ms` predates the given
/// scan start. Cascades by deleting orphaned tags for those segments/trips.
///
/// Trips referenced by any `timelapse_jobs` row (regardless of status)
/// are protected from GC even when their segments are gone — the
/// timelapse pre-render is treated as a long-term archive of the trip,
/// so the trip row must survive to keep the timelapse discoverable in
/// the UI. See `memory/project_timelapse_as_archive.md` for the design
/// intent.
/// Apply user-recorded trip merges to a freshly-grouped trip list.
/// `merges` maps absorbed_trip_id → primary_trip_id. Trips whose id
/// matches an absorbed key are relabeled to the primary; trips that
/// now share an id are coalesced (segments concatenated and sorted by
/// time, end_time stretched to span the union, camera_kind /
/// gps_supported inherited from the earliest constituent).
///
/// Returns a new `Vec<Trip>` rather than mutating in place because the
/// caller passes an immutable slice; the cost is one shallow clone per
/// trip, which is negligible (segment vectors aren't recursively
/// cloned by `Trip: Clone` — they move into the result).
pub(crate) fn apply_merges_to_trips(
    trips: &[Trip],
    merges: &HashMap<String, String>,
) -> Result<Vec<Trip>, AppError> {
    if merges.is_empty() {
        return Ok(trips.to_vec());
    }

    // Group by effective id (relabeled if absorbed).
    let mut groups: HashMap<String, Vec<Trip>> = HashMap::new();
    for trip in trips {
        let natural = trip.id.to_string();
        let effective = merges.get(&natural).cloned().unwrap_or(natural);
        groups.entry(effective).or_default().push(trip.clone());
    }

    let mut out: Vec<Trip> = Vec::with_capacity(groups.len());
    for (effective_id_str, mut bucket) in groups {
        let effective_id = Uuid::parse_str(&effective_id_str).map_err(|e| {
            AppError::Internal(format!(
                "manual_trip_merges contained non-UUID primary_trip_id {effective_id_str}: {e}"
            ))
        })?;

        // Earliest start wins (its segments come first; its
        // camera_kind / gps_supported flags are inherited).
        bucket.sort_by_key(|t| t.start_time);
        let mut base = bucket.remove(0);
        base.id = effective_id;
        for t in bucket {
            base.segments.extend(t.segments);
            if t.end_time > base.end_time {
                base.end_time = t.end_time;
            }
            // archive_only flag: if any constituent was archive-only
            // we'd lose the segment-bearing one's data, so OR is
            // wrong. Only mark archive_only if every constituent was;
            // since we extend segments, the merged trip is
            // archive_only iff all segments lists are empty (rare
            // post-merge). Recompute from segments below.
        }
        base.archive_only = base.segments.is_empty();
        base.segments.sort_by_key(|s| s.start_time);
        out.push(base);
    }

    out.sort_by_key(|t| t.start_time);
    Ok(out)
}

/// Persist `trips` with merge directives applied, GC stale rows, and
/// return the coalesced trip list the caller should hand to the frontend.
///
/// Returning the coalesced view is load-bearing: without it, scan_folder
/// would render the natural (unmerged) grouping to the UI while the DB
/// has the merge applied — the merged trip's primary keeps its segments
/// and timelapse_jobs while the natural absorbed trips appear as empty
/// of timelapses on the next folder reopen.
pub fn persist_and_gc(
    conn: &mut Connection,
    trips: &[Trip],
    scan_started_ms: i64,
) -> Result<Vec<Trip>, AppError> {
    let tx = conn.transaction()?;

    // Apply user merges before persisting. A merge directive says
    // "fold absorbed_trip_id into primary_trip_id"; we relabel and
    // coalesce the natural groups so every downstream upsert sees the
    // user's view of the world. Without this, the next rescan would
    // re-split any merged trips back to their natural form.
    let merges = crate::db::manual_trip_merges::list_merges(&tx)?;
    let coalesced = apply_merges_to_trips(trips, &merges)?;

    for trip in &coalesced {
        upsert_trip(&tx, trip, scan_started_ms)?;
        for seg in &trip.segments {
            upsert_segment(&tx, seg, &trip.id.to_string(), scan_started_ms)?;
        }
    }

    // GC: delete tags first (no FK cascade in sqlite without explicit pragma
    // on each connection), then segments, then trips. Trips referenced by
    // any timelapse_jobs row are kept (and their trip-level tags with them).
    tx.execute(
        "DELETE FROM tags WHERE segment_id IN (SELECT id FROM segments WHERE last_seen_ms < ?1)
            OR trip_id IN (
                SELECT id FROM trips
                WHERE last_seen_ms < ?1
                  AND id NOT IN (SELECT trip_id FROM timelapse_jobs)
            )",
        params![scan_started_ms],
    )?;
    tx.execute(
        "DELETE FROM scan_runs WHERE segment_id IN (SELECT id FROM segments WHERE last_seen_ms < ?1)",
        params![scan_started_ms],
    )?;
    tx.execute("DELETE FROM segments WHERE last_seen_ms < ?1", params![scan_started_ms])?;
    tx.execute(
        "DELETE FROM trips
         WHERE last_seen_ms < ?1
           AND id NOT IN (SELECT trip_id FROM timelapse_jobs)",
        params![scan_started_ms],
    )?;

    tx.commit()?;
    Ok(coalesced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::model::{derive_segment_id, derive_trip_id, Channel, Segment, Trip};
    use crate::scan::naming::CameraKind;
    use chrono::NaiveDate;

    fn sample_segment(master_path: &str, seconds: i64) -> Segment {
        let base = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let start_time = base + chrono::Duration::seconds(seconds);
        let id = derive_segment_id(master_path, start_time);
        Segment {
            id,
            start_time,
            duration_s: 60.0,
            is_event: false,
            channels: vec![Channel {
                label: "Front".into(),
                file_path: master_path.into(),
                width: None,
                height: None,
                fps_num: None,
                fps_den: None,
                codec: None,
                has_gpmd_track: false,
            }],
            camera_kind: CameraKind::WolfBox,
            gps_supported: true,
            size_bytes: None,
        }
    }

    fn sample_trip(segments: Vec<Segment>) -> Trip {
        let start = segments.first().unwrap().start_time;
        let end = segments.last().unwrap().start_time;
        let camera_kind = segments[0].camera_kind;
        let gps_supported = segments[0].gps_supported;
        Trip {
            id: derive_trip_id(segments[0].id),
            start_time: start,
            end_time: end,
            segments,
            camera_kind,
            gps_supported,
            archive_only: false,
        }
    }

    #[test]
    fn upsert_is_idempotent() {
        let db = open_in_memory().unwrap();
        let seg = sample_segment("C:/vids/a.mp4", 0);
        let trip = sample_trip(vec![seg.clone()]);

        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip.clone()], 1_000).unwrap();
        }
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip], 2_000).unwrap();
        }

        let conn = db.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM trips", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn size_bytes_round_trips_and_does_not_clobber_with_null() {
        // First write carries a real size; second write (e.g. a scan
        // that failed to stat the file) leaves size_bytes=None and must
        // NOT overwrite the previously-known value with NULL — that's
        // the COALESCE in upsert_segment's ON CONFLICT.
        let db = open_in_memory().unwrap();
        let mut seg = sample_segment("C:/vids/a.mp4", 0);
        seg.size_bytes = Some(12_345_678);
        let trip = sample_trip(vec![seg.clone()]);

        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, std::slice::from_ref(&trip), 1_000).unwrap();
        }

        // Verify the size persisted.
        {
            let conn = db.lock().unwrap();
            let stored: Option<i64> = conn
                .query_row(
                    "SELECT size_bytes FROM segments WHERE id = ?1",
                    params![seg.id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(stored, Some(12_345_678));
        }

        // Re-persist with size_bytes = None — must NOT clobber.
        let mut seg_unstat = seg.clone();
        seg_unstat.size_bytes = None;
        let trip_unstat = sample_trip(vec![seg_unstat]);
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip_unstat], 2_000).unwrap();
        }
        {
            let conn = db.lock().unwrap();
            let stored: Option<i64> = conn
                .query_row(
                    "SELECT size_bytes FROM segments WHERE id = ?1",
                    params![seg.id.to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                stored,
                Some(12_345_678),
                "a None-sized re-upsert must preserve the previously-known size",
            );
        }
    }

    #[test]
    fn gc_removes_segments_not_seen_in_latest_scan() {
        let db = open_in_memory().unwrap();
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let seg_b = sample_segment("C:/vids/b.mp4", 60);
        let trip = sample_trip(vec![seg_a.clone(), seg_b.clone()]);

        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip.clone()], 1_000).unwrap();
        }

        // Second scan sees only seg_a. seg_b should be gc'd.
        let trip2 = sample_trip(vec![seg_a]);
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip2], 2_000).unwrap();
        }

        let conn = db.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn trip_with_timelapse_job_survives_gc_when_segments_vanish() {
        let db = open_in_memory().unwrap();
        let seg = sample_segment("C:/vids/a.mp4", 0);
        let trip = sample_trip(vec![seg]);
        let trip_id = trip.id.to_string();

        // First scan persists the trip + segment.
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip], 1_000).unwrap();
        }

        // Simulate a completed timelapse encode: insert a row in
        // timelapse_jobs for this trip. This is what protects the trip
        // from GC after the source segments are gone.
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO timelapse_jobs
                    (trip_id, tier, channel, status, output_path, created_at_ms, completed_at_ms)
                 VALUES (?1, '8x', 'F', 'done', '/tl/8x_F.mp4', 1500, 1500)",
                params![trip_id],
            )
            .unwrap();
        }

        // Second scan sees no trips at all (e.g. SD card removed or
        // user pointed scan at a different folder). Without the
        // timelapse-jobs guard, the trip and its segment would be GC'd.
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[], 2_000).unwrap();
        }

        let conn = db.lock().unwrap();
        // Segments are still GC'd (the source file is gone) — this is correct.
        let segs: i64 = conn
            .query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(segs, 0, "segments should be GC'd when not seen");
        // Trip survives because of the timelapse job.
        let trips: i64 = conn
            .query_row("SELECT COUNT(*) FROM trips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(trips, 1, "trip with timelapse_jobs row must survive GC");
        // Timelapse job row untouched.
        let jobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM timelapse_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(jobs, 1);
    }

    #[test]
    fn trip_without_timelapse_job_is_gcd_when_segments_vanish() {
        let db = open_in_memory().unwrap();
        let seg = sample_segment("C:/vids/a.mp4", 0);
        let trip = sample_trip(vec![seg]);

        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip], 1_000).unwrap();
        }
        // Second scan sees no trips, no archive — trip should be GC'd
        // exactly as before.
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[], 2_000).unwrap();
        }

        let conn = db.lock().unwrap();
        let trips: i64 = conn
            .query_row("SELECT COUNT(*) FROM trips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(trips, 0);
    }

    // ── apply_merges_to_trips ────────────────────────────────────────

    #[test]
    fn apply_merges_noop_when_map_empty() {
        let s = sample_segment("C:/vids/a.mp4", 0);
        let trip = sample_trip(vec![s]);
        let trip_id = trip.id;
        let merges = HashMap::new();
        let out = apply_merges_to_trips(&[trip], &merges).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, trip_id);
    }

    #[test]
    fn apply_merges_folds_absorbed_into_primary() {
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let seg_b = sample_segment("C:/vids/b.mp4", 600);
        let trip_a = sample_trip(vec![seg_a]);
        let trip_b = sample_trip(vec![seg_b]);
        let primary = trip_a.id;
        let absorbed = trip_b.id;

        let mut merges = HashMap::new();
        merges.insert(absorbed.to_string(), primary.to_string());

        let out = apply_merges_to_trips(&[trip_a, trip_b], &merges).unwrap();
        assert_eq!(out.len(), 1, "two trips should fold into one");
        assert_eq!(out[0].id, primary);
        assert_eq!(out[0].segments.len(), 2);
        // Earliest-first ordering preserved across the merge.
        assert!(out[0].segments[0].start_time < out[0].segments[1].start_time);
    }

    #[test]
    fn apply_merges_coalesces_three_into_one() {
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let seg_b = sample_segment("C:/vids/b.mp4", 600);
        let seg_c = sample_segment("C:/vids/c.mp4", 1200);
        let trip_a = sample_trip(vec![seg_a]);
        let trip_b = sample_trip(vec![seg_b]);
        let trip_c = sample_trip(vec![seg_c]);
        let primary = trip_a.id;

        let mut merges = HashMap::new();
        merges.insert(trip_b.id.to_string(), primary.to_string());
        merges.insert(trip_c.id.to_string(), primary.to_string());

        let out = apply_merges_to_trips(&[trip_a, trip_b, trip_c], &merges).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, primary);
        assert_eq!(out[0].segments.len(), 3);
    }

    #[test]
    fn apply_merges_skips_orphaned_directives() {
        // Merge directive references a primary that doesn't exist in
        // the natural groups — common after a first-segment deletion
        // wiped out the primary's natural form. The absorbed trip
        // gets relabeled to the orphan primary id; nothing crashes.
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let trip_a = sample_trip(vec![seg_a]);
        let absorbed = trip_a.id;
        let orphan_primary = Uuid::from_bytes([0xFF; 16]);

        let mut merges = HashMap::new();
        merges.insert(absorbed.to_string(), orphan_primary.to_string());

        let out = apply_merges_to_trips(&[trip_a], &merges).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].id, orphan_primary,
            "absorbed trip should be relabeled even when primary isn't in the natural set",
        );
    }

    #[test]
    fn apply_merges_persists_across_persist_and_gc() {
        // End-to-end: insert a merge directive, then run persist_and_gc.
        // The segments should land under the primary trip ID.
        let db = open_in_memory().unwrap();
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let seg_b = sample_segment("C:/vids/b.mp4", 600);
        let trip_a = sample_trip(vec![seg_a]);
        let trip_b = sample_trip(vec![seg_b]);
        let primary = trip_a.id;
        let absorbed = trip_b.id;

        {
            let conn = db.lock().unwrap();
            crate::db::manual_trip_merges::insert_merge(
                &conn, primary, absorbed, 500,
            )
            .unwrap();
        }
        {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip_a, trip_b], 1_000).unwrap();
        }
        let conn = db.lock().unwrap();
        let n_trips: i64 = conn
            .query_row("SELECT COUNT(*) FROM trips", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n_trips, 1, "absorbed trip's row must not be persisted");
        let n_segs_under_primary: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE trip_id = ?1",
                params![primary.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n_segs_under_primary, 2,
            "both segments should now live under the primary trip",
        );
    }

    #[test]
    fn persist_and_gc_returns_merged_trips_for_caller() {
        // The frontend renders whatever scan_folder returns. If
        // persist_and_gc applied the merge to the DB but the natural
        // (unmerged) trips were handed back, the next folder reopen
        // would show three separate trips while their timelapse_jobs
        // sit under the merged primary's id. Lock the contract here.
        let db = open_in_memory().unwrap();
        let seg_a = sample_segment("C:/vids/a.mp4", 0);
        let seg_b = sample_segment("C:/vids/b.mp4", 600);
        let trip_a = sample_trip(vec![seg_a]);
        let trip_b = sample_trip(vec![seg_b]);
        let primary = trip_a.id;
        let absorbed = trip_b.id;

        {
            let conn = db.lock().unwrap();
            crate::db::manual_trip_merges::insert_merge(
                &conn, primary, absorbed, 500,
            )
            .unwrap();
        }
        let returned = {
            let mut conn = db.lock().unwrap();
            persist_and_gc(&mut conn, &[trip_a, trip_b], 1_000).unwrap()
        };

        assert_eq!(returned.len(), 1, "two natural trips should fold into one");
        assert_eq!(returned[0].id, primary);
        assert_eq!(returned[0].segments.len(), 2);
    }
}
