use std::collections::HashMap;
use tauri::State;
use uuid::Uuid;

use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::model::Trip;
use crate::tags::commands::DeleteFailure;
use crate::trips::merge::{
    self, MergeReport, TimelapseMergeAssessment, TimelapseMergeStrategy,
};

/// Return every trip whose source segments have all been deleted but
/// which still has at least one `timelapse_jobs` row. The frontend
/// merges these with the scan-derived trip list so the timelapse
/// archive is always reachable in the sidebar — segment deletion is
/// allowed to free disk without making the trip vanish from the UI.
#[tauri::command]
pub async fn list_archive_only_trips(
    db: State<'_, DbHandle>,
) -> Result<Vec<Trip>, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::segments::list_archive_only_trips(&conn)
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTripReport {
    /// How many source-segment channel files were moved to trash.
    pub segment_files_trashed: usize,
    /// How many timelapse pre-render `.mp4` files were moved to trash.
    pub timelapse_files_trashed: usize,
    /// `timelapse_jobs` rows removed (one per (tier, channel)).
    pub timelapse_jobs_removed: usize,
    /// Whether the trip row itself was deleted. Always `true` on success;
    /// kept in the payload so the frontend can sanity-check its store
    /// update against the backend's truth.
    pub trip_removed: bool,
    pub failures: Vec<DeleteFailure>,
}

/// Wholesale "delete this entire trip" — moves every source MP4 and
/// every timelapse pre-render to the OS trash, then removes all
/// associated DB rows (`tags`, `scan_runs`, `segments`, `timelapse_jobs`,
/// `trips`). This is the only path that ever removes a timelapse; the
/// per-segment delete leaves the archive intact by design.
///
/// `in_memory_paths` maps segment_id → channel paths (the frontend
/// knows the full channel list from its in-memory trips). Segments
/// not in that map fall back to the `master_path` stored on their row.
#[tauri::command]
pub async fn delete_trip(
    trip_id: String,
    in_memory_paths: HashMap<String, Vec<String>>,
    db: State<'_, DbHandle>,
) -> Result<DeleteTripReport, AppError> {
    let mut report = DeleteTripReport::default();

    // Phase 1: gather every file path we need to trash. Done under a
    // single DB read so the snapshot is consistent.
    let mut segment_paths: Vec<String> = Vec::new();
    let mut timelapse_paths: Vec<String> = Vec::new();
    let mut segment_ids: Vec<String> = Vec::new();
    {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;

        // Source segments. Use in_memory_paths for the full channel
        // list when present, fall back to master_path otherwise.
        let mut stmt =
            conn.prepare("SELECT id, master_path FROM segments WHERE trip_id = ?1")?;
        let rows = stmt.query_map([&trip_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (seg_id, master) = row?;
            if let Some(paths) = in_memory_paths.get(&seg_id) {
                segment_paths.extend(paths.iter().cloned());
            } else {
                segment_paths.push(master);
            }
            segment_ids.push(seg_id);
        }

        // Timelapse outputs. NULL output_path means the encode never
        // produced a file (pending/failed) — nothing to trash, but the
        // row still gets deleted in Phase 3.
        let mut stmt = conn.prepare(
            "SELECT output_path FROM timelapse_jobs
             WHERE trip_id = ?1 AND output_path IS NOT NULL",
        )?;
        let rows = stmt.query_map([&trip_id], |r| r.get::<_, String>(0))?;
        for row in rows {
            timelapse_paths.push(row?);
        }
    }

    // Phase 2: trash files. Failures are collected per-file but don't
    // abort the operation — partial cleanup is better than an
    // all-or-nothing rollback that leaves trash in two places.
    for path_str in &segment_paths {
        match try_trash(path_str) {
            TrashOutcome::Trashed => report.segment_files_trashed += 1,
            TrashOutcome::AlreadyGone => {}
            TrashOutcome::Failed(message) => report.failures.push(DeleteFailure {
                path: path_str.clone(),
                message,
            }),
        }
    }
    for path_str in &timelapse_paths {
        match try_trash(path_str) {
            TrashOutcome::Trashed => report.timelapse_files_trashed += 1,
            TrashOutcome::AlreadyGone => {}
            TrashOutcome::Failed(message) => report.failures.push(DeleteFailure {
                path: path_str.clone(),
                message,
            }),
        }
    }

    // Phase 3: drop every DB row for this trip in one transaction.
    {
        let mut conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let outcome = purge_trip_rows(&mut conn, &trip_id)?;
        report.timelapse_jobs_removed = outcome.timelapse_jobs_removed;
        report.trip_removed = outcome.trip_removed;
    }

    let _ = segment_ids; // currently unused; reserved for future per-id reporting
    Ok(report)
}

#[derive(Debug)]
struct PurgeOutcome {
    timelapse_jobs_removed: usize,
    trip_removed: bool,
}

/// Drop every DB row tied to a trip in a single transaction. Tags
/// first (no FK cascade in our schema), then scan_runs, segments,
/// timelapse_jobs, and finally the trip row itself. Pulled out of
/// `delete_trip` so tests can exercise the DB-only path without
/// involving the OS trash.
fn purge_trip_rows(
    conn: &mut rusqlite::Connection,
    trip_id: &str,
) -> Result<PurgeOutcome, AppError> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM tags WHERE trip_id = ?1
            OR segment_id IN (SELECT id FROM segments WHERE trip_id = ?1)",
        rusqlite::params![trip_id],
    )?;
    tx.execute(
        "DELETE FROM scan_runs WHERE segment_id IN
            (SELECT id FROM segments WHERE trip_id = ?1)",
        rusqlite::params![trip_id],
    )?;
    tx.execute(
        "DELETE FROM segments WHERE trip_id = ?1",
        rusqlite::params![trip_id],
    )?;
    let jobs_n = tx.execute(
        "DELETE FROM timelapse_jobs WHERE trip_id = ?1",
        rusqlite::params![trip_id],
    )?;
    let trip_n = tx.execute(
        "DELETE FROM trips WHERE id = ?1",
        rusqlite::params![trip_id],
    )?;
    tx.commit()?;
    Ok(PurgeOutcome {
        timelapse_jobs_removed: jobs_n,
        trip_removed: trip_n > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use rusqlite::params;

    /// Seed a trip with one segment, one user tag, one trip-level tag,
    /// one scan_run, and one done timelapse_jobs row. Segment id is
    /// derived from `trip_id` so the helper can be called for multiple
    /// trips in the same DB without UNIQUE-constraint collisions.
    fn seed_trip(conn: &rusqlite::Connection, trip_id: &str) {
        let seg_id = format!("seg-{trip_id}");
        conn.execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
             VALUES (?1, 0, 60000, 'wolfBox', 1, 1000)",
            params![trip_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments
                (id, trip_id, start_time_ms, duration_s, master_path, is_event,
                 camera_kind, gps_supported, last_seen_ms)
             VALUES (?1, ?2, 0, 60.0, '/v/a.mp4', 0, 'wolfBox', 1, 1000)",
            params![&seg_id, trip_id],
        )
        .unwrap();
        // Trip-level tag (note) and segment-level user tag (keep).
        conn.execute(
            "INSERT INTO tags (segment_id, trip_id, name, category, source, created_ms)
             VALUES (NULL, ?1, 'note', 'user', 'user', 1000)",
            params![trip_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tags (segment_id, trip_id, name, category, source, created_ms)
             VALUES (?1, NULL, 'keep', 'user', 'user', 1000)",
            params![&seg_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (segment_id, scan_id, version, ran_at_ms, status)
             VALUES (?1, 'motion', 1, 1000, 'done')",
            params![&seg_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms, completed_at_ms)
             VALUES (?1, '8x', 'F', 'done', '/tl/8x_F.mp4', 1500, 1500)",
            params![trip_id],
        )
        .unwrap();
    }

    fn count(conn: &rusqlite::Connection, sql: &str, trip_id: &str) -> i64 {
        conn.query_row(sql, params![trip_id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn purge_trip_rows_removes_everything_for_one_trip() {
        let db = open_in_memory().unwrap();
        let mut conn = db.lock().unwrap();
        seed_trip(&conn, "trip-A");
        seed_trip(&conn, "trip-B");

        let out = purge_trip_rows(&mut conn, "trip-A").unwrap();
        assert!(out.trip_removed);
        assert_eq!(out.timelapse_jobs_removed, 1);

        // trip-A is gone end-to-end.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM trips WHERE id = ?1", "trip-A"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM segments WHERE trip_id = ?1", "trip-A"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM timelapse_jobs WHERE trip_id = ?1", "trip-A"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM tags WHERE trip_id = ?1", "trip-A"), 0);
        // Segment-level tags and scan_runs for trip-A's segment followed
        // it into oblivion.
        let seg_tag_n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tags WHERE segment_id = 'seg-trip-A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seg_tag_n, 0);
        let scan_run_n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scan_runs WHERE segment_id = 'seg-trip-A'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scan_run_n, 0);

        // trip-B is still intact.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM trips WHERE id = ?1", "trip-B"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM timelapse_jobs WHERE trip_id = ?1", "trip-B"), 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM segments WHERE trip_id = ?1", "trip-B"),
            1
        );
    }

    #[test]
    fn purge_trip_rows_handles_archive_only_trip() {
        // Archive-only: trip has timelapse_jobs but no segments. Purge
        // must succeed and return trip_removed=true / one job removed.
        let db = open_in_memory().unwrap();
        let mut conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind, gps_supported, last_seen_ms)
             VALUES ('trip-A', 0, 60000, 'wolfBox', 1, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms, completed_at_ms)
             VALUES ('trip-A', '8x', 'F', 'done', '/tl/8x_F.mp4', 1500, 1500)",
            [],
        )
        .unwrap();

        let out = purge_trip_rows(&mut conn, "trip-A").unwrap();
        assert!(out.trip_removed);
        assert_eq!(out.timelapse_jobs_removed, 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM trips WHERE id = ?1", "trip-A"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM timelapse_jobs WHERE trip_id = ?1", "trip-A"), 0);
    }
}

/// Report what's possible with the existing timelapse outputs of the
/// to-be-merged trips. Returns `has_any_timelapses=false` (and an empty
/// `tuples` list) when the frontend can skip the dialog and merge
/// silently. Otherwise enumerates each (tier, channel) tuple at least
/// one source has, classifying as Concatenable (all sources have it)
/// or PartialOutputs (some do, some don't).
#[tauri::command]
pub async fn assess_trip_merge(
    primary_trip_id: String,
    absorbed_trip_ids: Vec<String>,
    db: State<'_, DbHandle>,
) -> Result<TimelapseMergeAssessment, AppError> {
    let primary = parse_trip_uuid(&primary_trip_id)?;
    let absorbed = parse_trip_uuids(&absorbed_trip_ids)?;
    merge::assess_timelapse_merge(&db, primary, &absorbed)
}

/// Perform the merge: rewrite segments + tags + timelapse_jobs to point
/// at `primary_trip_id`, optionally concat existing timelapse outputs
/// per `strategy`, record one row per absorbed trip in
/// `manual_trip_merges` so the merge survives a folder rescan, and
/// rebuild the primary's `trips` row to span the union.
#[tauri::command]
pub async fn merge_trips(
    primary_trip_id: String,
    absorbed_trip_ids: Vec<String>,
    strategy: TimelapseMergeStrategy,
    db: State<'_, DbHandle>,
) -> Result<MergeReport, AppError> {
    let primary = parse_trip_uuid(&primary_trip_id)?;
    let absorbed = parse_trip_uuids(&absorbed_trip_ids)?;
    merge::merge_trips(&db, primary, &absorbed, strategy)
}

fn parse_trip_uuid(s: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(s)
        .map_err(|e| AppError::Internal(format!("invalid trip UUID {s:?}: {e}")))
}

fn parse_trip_uuids(strs: &[String]) -> Result<Vec<Uuid>, AppError> {
    strs.iter().map(|s| parse_trip_uuid(s)).collect()
}

enum TrashOutcome {
    Trashed,
    AlreadyGone,
    Failed(String),
}

fn try_trash(path_str: &str) -> TrashOutcome {
    let path = std::path::Path::new(path_str);
    if !path.exists() {
        return TrashOutcome::AlreadyGone;
    }
    match trash::delete(path) {
        Ok(_) => TrashOutcome::Trashed,
        Err(e) => TrashOutcome::Failed(e.to_string()),
    }
}
