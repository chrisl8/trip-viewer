//! Library-wide storage summary. Powers the sidebar header line
//! "X GB used · Y GB reclaimable" and the click-to-filter list of
//! reclaimable trips.
//!
//! "Reclaimable" = a trip has at least one done timelapse_jobs row AND
//! still has source segments on disk. Deleting the originals on such a
//! trip leaves the timelapse archive in place — that's the disk-reclaim
//! workflow this surface exists to advertise.

use rusqlite::params;
use serde::Serialize;
use tauri::State;

use crate::db::{self, DbHandle};
use crate::error::AppError;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStorageSummary {
    /// Sum of all known segment sizes plus all known timelapse output
    /// sizes. NULLs (unknown) contribute zero.
    pub total_bytes: i64,
    /// Originals (segments) only. Useful for the "X GB of originals" UI.
    pub originals_bytes: i64,
    /// Timelapse outputs only. Useful for the "Y MB of archive" UI.
    pub timelapse_bytes: i64,
    /// Bytes that would be freed by deleting originals on every
    /// reclaimable trip (see module-level comment for the definition).
    pub reclaimable_bytes: i64,
    /// Trip ids whose originals can be safely deleted right now —
    /// returned alongside the totals so the click-to-filter list view
    /// doesn't need a second roundtrip.
    pub reclaimable_trip_ids: Vec<String>,
}

#[tauri::command]
pub async fn get_library_storage_summary(
    db: State<'_, DbHandle>,
) -> Result<LibraryStorageSummary, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    compute_summary(&conn)
}

pub(crate) fn compute_summary(
    conn: &rusqlite::Connection,
) -> Result<LibraryStorageSummary, AppError> {
    let originals_bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM segments",
        [],
        |r| r.get(0),
    )?;
    let timelapse_bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(output_size_bytes), 0) FROM timelapse_jobs",
        [],
        |r| r.get(0),
    )?;
    let reclaimable_bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(s.size_bytes), 0)
         FROM segments s
         WHERE EXISTS (
             SELECT 1 FROM timelapse_jobs j
             WHERE j.trip_id = s.trip_id AND j.status = ?1
         )",
        params![db::timelapse_jobs::STATUS_DONE],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT s.trip_id
         FROM segments s
         WHERE EXISTS (
             SELECT 1 FROM timelapse_jobs j
             WHERE j.trip_id = s.trip_id AND j.status = ?1
         )
         ORDER BY s.trip_id",
    )?;
    let mapped =
        stmt.query_map(params![db::timelapse_jobs::STATUS_DONE], |r| r.get::<_, String>(0))?;
    let mut reclaimable_trip_ids = Vec::new();
    for row in mapped {
        reclaimable_trip_ids.push(row?);
    }
    Ok(LibraryStorageSummary {
        total_bytes: originals_bytes + timelapse_bytes,
        originals_bytes,
        timelapse_bytes,
        reclaimable_bytes,
        reclaimable_trip_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use rusqlite::params;

    fn insert_trip(conn: &rusqlite::Connection, trip_id: &str) {
        conn.execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
             VALUES (?1, 0, 60000, 'wolfBox', 1, 1000)",
            params![trip_id],
        )
        .unwrap();
    }

    fn insert_segment(
        conn: &rusqlite::Connection,
        seg_id: &str,
        trip_id: &str,
        size: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO segments
                (id, trip_id, start_time_ms, duration_s, master_path, is_event,
                 camera_kind, gps_supported, last_seen_ms, size_bytes)
             VALUES (?1, ?2, 0, 60.0, '/v/x.mp4', 0, 'wolfBox', 1, 1000, ?3)",
            params![seg_id, trip_id, size],
        )
        .unwrap();
    }

    fn insert_done_timelapse(
        conn: &rusqlite::Connection,
        trip_id: &str,
        tier: &str,
        channel: &str,
        size: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path,
                 created_at_ms, completed_at_ms, output_size_bytes)
             VALUES (?1, ?2, ?3, 'done', '/tl/x.mp4', 1500, 1500, ?4)",
            params![trip_id, tier, channel, size],
        )
        .unwrap();
    }

    #[test]
    fn empty_library_is_all_zero() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let s = compute_summary(&conn).unwrap();
        assert_eq!(s.total_bytes, 0);
        assert_eq!(s.originals_bytes, 0);
        assert_eq!(s.timelapse_bytes, 0);
        assert_eq!(s.reclaimable_bytes, 0);
        assert!(s.reclaimable_trip_ids.is_empty());
    }

    #[test]
    fn totals_sum_segments_and_timelapses_with_nulls_as_zero() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        insert_trip(&conn, "t1");
        insert_segment(&conn, "s1", "t1", Some(1_000));
        insert_segment(&conn, "s2", "t1", Some(2_000));
        insert_segment(&conn, "s3", "t1", None); // unknown — contributes 0
        insert_done_timelapse(&conn, "t1", "8x", "F", Some(500));
        insert_done_timelapse(&conn, "t1", "8x", "I", None);

        let s = compute_summary(&conn).unwrap();
        assert_eq!(s.originals_bytes, 3_000);
        assert_eq!(s.timelapse_bytes, 500);
        assert_eq!(s.total_bytes, 3_500);
    }

    #[test]
    fn reclaimable_includes_only_trips_with_at_least_one_done_timelapse() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();

        // Trip A: has a done timelapse + originals → reclaimable.
        insert_trip(&conn, "A");
        insert_segment(&conn, "sa1", "A", Some(10_000));
        insert_segment(&conn, "sa2", "A", Some(5_000));
        insert_done_timelapse(&conn, "A", "8x", "F", Some(400));

        // Trip B: no timelapse at all → not reclaimable, but still
        // contributes to originals_bytes / total_bytes.
        insert_trip(&conn, "B");
        insert_segment(&conn, "sb1", "B", Some(20_000));

        // Trip C: only a pending timelapse → not reclaimable.
        insert_trip(&conn, "C");
        insert_segment(&conn, "sc1", "C", Some(7_000));
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms)
             VALUES ('C', '8x', 'F', 'pending', NULL, 1500)",
            [],
        )
        .unwrap();

        let s = compute_summary(&conn).unwrap();
        assert_eq!(s.reclaimable_trip_ids, vec!["A".to_string()]);
        assert_eq!(s.reclaimable_bytes, 15_000);
        assert_eq!(s.originals_bytes, 42_000);
        assert_eq!(s.timelapse_bytes, 400);
    }

    #[test]
    fn archive_only_trips_do_not_appear_as_reclaimable() {
        // Archive-only = timelapse exists but segments are gone. Nothing
        // to reclaim — originals are already deleted.
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        insert_trip(&conn, "archived");
        insert_done_timelapse(&conn, "archived", "8x", "F", Some(800));

        let s = compute_summary(&conn).unwrap();
        assert!(s.reclaimable_trip_ids.is_empty());
        assert_eq!(s.reclaimable_bytes, 0);
        assert_eq!(s.timelapse_bytes, 800);
        assert_eq!(s.total_bytes, 800);
    }
}
