use rusqlite::{params, Connection};

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
    let camera_kind = match seg.camera_kind {
        crate::scan::naming::CameraKind::WolfBox => "wolfBox",
        crate::scan::naming::CameraKind::Thinkware => "thinkware",
        crate::scan::naming::CameraKind::Miltona => "miltona",
        crate::scan::naming::CameraKind::Generic => "generic",
    };
    conn.execute(
        "INSERT INTO segments (id, trip_id, start_time_ms, duration_s, master_path, is_event, camera_kind, gps_supported, last_seen_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
            trip_id = excluded.trip_id,
            start_time_ms = excluded.start_time_ms,
            duration_s = excluded.duration_s,
            master_path = excluded.master_path,
            is_event = excluded.is_event,
            camera_kind = excluded.camera_kind,
            gps_supported = excluded.gps_supported,
            last_seen_ms = excluded.last_seen_ms",
        params![
            seg.id.to_string(),
            trip_id,
            seg.start_time.and_utc().timestamp_millis(),
            seg.duration_s,
            master_path,
            seg.is_event as i32,
            camera_kind,
            seg.gps_supported as i32,
            now_ms,
        ],
    )?;
    Ok(())
}

pub fn upsert_trip(conn: &Connection, trip: &Trip, now_ms: i64) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO trips (id, start_time_ms, end_time_ms, last_seen_ms)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
            start_time_ms = excluded.start_time_ms,
            end_time_ms = excluded.end_time_ms,
            last_seen_ms = excluded.last_seen_ms",
        params![
            trip.id.to_string(),
            trip.start_time.and_utc().timestamp_millis(),
            trip.end_time.and_utc().timestamp_millis(),
            now_ms,
        ],
    )?;
    Ok(())
}

/// Upsert all trips and their segments in a single transaction, then
/// delete any segment/trip rows whose `last_seen_ms` predates the given
/// scan start. Cascades by deleting orphaned tags for those segments/trips.
pub fn persist_and_gc(conn: &mut Connection, trips: &[Trip], scan_started_ms: i64) -> Result<(), AppError> {
    let tx = conn.transaction()?;
    for trip in trips {
        upsert_trip(&tx, trip, scan_started_ms)?;
        for seg in &trip.segments {
            upsert_segment(&tx, seg, &trip.id.to_string(), scan_started_ms)?;
        }
    }

    // GC: delete tags first (no FK cascade in sqlite without explicit pragma
    // on each connection), then segments, then trips.
    tx.execute(
        "DELETE FROM tags WHERE segment_id IN (SELECT id FROM segments WHERE last_seen_ms < ?1)
            OR trip_id IN (SELECT id FROM trips WHERE last_seen_ms < ?1)",
        params![scan_started_ms],
    )?;
    tx.execute(
        "DELETE FROM scan_runs WHERE segment_id IN (SELECT id FROM segments WHERE last_seen_ms < ?1)",
        params![scan_started_ms],
    )?;
    tx.execute("DELETE FROM segments WHERE last_seen_ms < ?1", params![scan_started_ms])?;
    tx.execute("DELETE FROM trips WHERE last_seen_ms < ?1", params![scan_started_ms])?;

    tx.commit()?;
    Ok(())
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
        }
    }

    fn sample_trip(segments: Vec<Segment>) -> Trip {
        let start = segments.first().unwrap().start_time;
        let end = segments.last().unwrap().start_time;
        Trip {
            id: derive_trip_id(segments[0].id),
            start_time: start,
            end_time: end,
            segments,
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
}
