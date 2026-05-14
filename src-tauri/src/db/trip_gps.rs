//! CRUD for the `trip_gps` table. One row per trip; the payload is a
//! JSON-serialized `Vec<GpsPoint>` with trip-stitched `t_offset_s` (i.e.
//! cumulative across segments, matching what `MapPanel` expects in
//! tier/timelapse mode). Written once after a successful per-trip GPS
//! stitch in the timelapse encoder, read at trip-select time so the map
//! and speed graph survive a subsequent "Delete originals".

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;
use crate::model::GpsPoint;

pub fn upsert(
    conn: &Connection,
    trip_id: &str,
    points: &[GpsPoint],
    parser_version: i32,
) -> Result<(), AppError> {
    let json = serde_json::to_string(points)
        .map_err(|e| AppError::Internal(format!("trip_gps serialize: {e}")))?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO trip_gps (trip_id, points_json, point_count, parser_version, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(trip_id) DO UPDATE SET
            points_json    = excluded.points_json,
            point_count    = excluded.point_count,
            parser_version = excluded.parser_version,
            created_at_ms  = excluded.created_at_ms",
        params![trip_id, json, points.len() as i64, parser_version, now],
    )?;
    Ok(())
}

pub fn load(conn: &Connection, trip_id: &str) -> Result<Option<Vec<GpsPoint>>, AppError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT points_json FROM trip_gps WHERE trip_id = ?1",
            params![trip_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(json) = row else { return Ok(None) };
    let points: Vec<GpsPoint> = serde_json::from_str(&json)
        .map_err(|e| AppError::Internal(format!("trip_gps deserialize: {e}")))?;
    Ok(Some(points))
}

#[allow(dead_code)] // reserved for an explicit invalidation path
pub fn delete(conn: &Connection, trip_id: &str) -> Result<usize, AppError> {
    Ok(conn.execute(
        "DELETE FROM trip_gps WHERE trip_id = ?1",
        params![trip_id],
    )?)
}

/// Returns true when the trip already has an archived GPS row at or
/// above `min_parser_version`. The encoder uses this to skip redundant
/// writes on timelapse re-runs; the startup backfill uses it to bound
/// its work to the trips that actually need re-extraction.
pub fn has_current(
    conn: &Connection,
    trip_id: &str,
    min_parser_version: i32,
) -> Result<bool, AppError> {
    let row: Option<i64> = conn
        .query_row(
            "SELECT parser_version FROM trip_gps WHERE trip_id = ?1",
            params![trip_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(row.map(|pv| pv as i32 >= min_parser_version).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn sample_points() -> Vec<GpsPoint> {
        vec![
            GpsPoint {
                t_offset_s: 0.0,
                lat: 45.123456,
                lon: -93.654321,
                speed_mps: 12.5,
                heading_deg: 87.4,
                altitude_m: 245.0,
                fix_ok: true,
            },
            GpsPoint {
                t_offset_s: 1.0,
                lat: 45.123567,
                lon: -93.654211,
                speed_mps: 12.7,
                heading_deg: 87.5,
                altitude_m: 245.1,
                fix_ok: true,
            },
        ]
    }

    fn insert_trip(db: &super::super::DbHandle, trip_id: &str) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
             VALUES (?1, 0, 60000, 'wolfBox', 1, 0)",
            params![trip_id],
        )
        .unwrap();
    }

    #[test]
    fn upsert_creates_row() {
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        upsert(&conn, "trip-1", &sample_points(), 1).unwrap();
        let loaded = load(&conn, "trip-1").unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn upsert_overwrites_existing() {
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        upsert(&conn, "trip-1", &sample_points(), 1).unwrap();
        let mut grew = sample_points();
        grew.push(GpsPoint {
            t_offset_s: 2.0,
            lat: 45.124,
            lon: -93.654,
            speed_mps: 13.0,
            heading_deg: 88.0,
            altitude_m: 245.2,
            fix_ok: true,
        });
        upsert(&conn, "trip-1", &grew, 2).unwrap();
        let loaded = load(&conn, "trip-1").unwrap().unwrap();
        assert_eq!(loaded.len(), 3);
        let pv: i64 = conn
            .query_row(
                "SELECT parser_version FROM trip_gps WHERE trip_id = ?1",
                params!["trip-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pv, 2);
    }

    #[test]
    fn load_returns_none_when_missing() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        assert!(load(&conn, "ghost").unwrap().is_none());
    }

    #[test]
    fn round_trip_preserves_every_gpspoint_field() {
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        let original = sample_points();
        upsert(&conn, "trip-1", &original, 1).unwrap();
        let loaded = load(&conn, "trip-1").unwrap().unwrap();
        assert_eq!(loaded.len(), original.len());
        for (a, b) in original.iter().zip(loaded.iter()) {
            assert_eq!(a.t_offset_s, b.t_offset_s);
            assert_eq!(a.lat, b.lat);
            assert_eq!(a.lon, b.lon);
            assert_eq!(a.speed_mps, b.speed_mps);
            assert_eq!(a.heading_deg, b.heading_deg);
            assert_eq!(a.altitude_m, b.altitude_m);
            assert_eq!(a.fix_ok, b.fix_ok);
        }
    }

    #[test]
    fn has_current_handles_three_cases() {
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        assert!(!has_current(&conn, "trip-1", 1).unwrap(), "missing row");
        upsert(&conn, "trip-1", &sample_points(), 2).unwrap();
        assert!(has_current(&conn, "trip-1", 2).unwrap(), "exactly current");
        assert!(has_current(&conn, "trip-1", 1).unwrap(), "above min");
        assert!(!has_current(&conn, "trip-1", 3).unwrap(), "below current");
    }

    #[test]
    fn delete_removes_row() {
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        upsert(&conn, "trip-1", &sample_points(), 1).unwrap();
        let n = delete(&conn, "trip-1").unwrap();
        assert_eq!(n, 1);
        assert!(load(&conn, "trip-1").unwrap().is_none());
    }

    #[test]
    fn fk_cascade_on_trip_delete() {
        // Migration 0012 declares trip_gps.trip_id REFERENCES trips(id)
        // ON DELETE CASCADE. open_in_memory now sets foreign_keys=ON so
        // this fires identically to production.
        let db = open_in_memory().unwrap();
        insert_trip(&db, "trip-1");
        let conn = db.lock().unwrap();
        upsert(&conn, "trip-1", &sample_points(), 1).unwrap();
        conn.execute("DELETE FROM trips WHERE id = ?1", params!["trip-1"])
            .unwrap();
        assert!(
            load(&conn, "trip-1").unwrap().is_none(),
            "trip_gps row should cascade-delete when its trip row is removed"
        );
    }
}
