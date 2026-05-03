use std::collections::HashMap;
use tauri::State;

use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::tags::vocabulary::{UserApplicableTag, USER_APPLICABLE_TAGS};
use crate::tags::Tag;

/// Return the developer-curated list of tags the user can apply from
/// the player tag bar and the Review view dropdowns. Fetched once at
/// app startup and cached in the frontend store.
#[tauri::command]
pub async fn list_user_applicable_tags() -> Vec<UserApplicableTag> {
    USER_APPLICABLE_TAGS.to_vec()
}

/// Return all tags attached to a trip: both trip-level tags and tags on
/// any segment belonging to the trip.
#[tauri::command]
pub async fn get_tags_for_trip(
    trip_id: String,
    db: State<'_, DbHandle>,
) -> Result<Vec<Tag>, AppError> {
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::tags::tags_for_trip(&conn, &trip_id)
}

/// Library-wide sidebar aggregation: `{ trip_id: { tag_name: count } }`.
/// Returned as nested maps rather than a flat list so the frontend can
/// render per-trip badges without client-side grouping.
#[tauri::command]
pub async fn get_tag_counts_by_trip(
    db: State<'_, DbHandle>,
) -> Result<HashMap<String, HashMap<String, i64>>, AppError> {
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::tags::all_trip_tag_counts(&conn)
}

/// Every tag in the DB. Used by the Review view for full-library
/// faceted browsing. Cheap compared to per-trip reloads at the scale
/// we expect (tens of thousands of tags, not millions).
#[tauri::command]
pub async fn get_all_tags(
    db: State<'_, DbHandle>,
) -> Result<Vec<Tag>, AppError> {
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::tags::all_tags(&conn)
}

/// Apply a user-source tag (e.g. `keep`) to a set of segments. Dedups:
/// segments that already have this user tag are skipped. Returns the
/// number of new tag rows inserted.
#[tauri::command]
pub async fn add_user_tag(
    segment_ids: Vec<String>,
    name: String,
    note: Option<String>,
    db: State<'_, DbHandle>,
) -> Result<usize, AppError> {
    let mut conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::tags::insert_user_tag_for_segments(&mut conn, &segment_ids, &name, note.as_deref())
}

/// Remove a specific user-source tag from a set of segments. Returns
/// the number of rows deleted.
#[tauri::command]
pub async fn remove_user_tag(
    segment_ids: Vec<String>,
    name: String,
    db: State<'_, DbHandle>,
) -> Result<usize, AppError> {
    let mut conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::tags::remove_user_tag_for_segments(&mut conn, &segment_ids, &name)
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    pub segments_removed: usize,
    pub files_trashed: usize,
    pub failures: Vec<DeleteFailure>,
    /// Segment IDs that were converted to tombstones (kept on the
    /// timeline as hatched gaps because the trip has a completed
    /// timelapse archive). Disjoint from any IDs implicit in
    /// `segments_removed` minus this list — those were hard-deleted.
    pub tombstoned_segment_ids: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFailure {
    pub path: String,
    pub message: String,
}

/// Move every channel file for the given segments to the OS trash,
/// then delete the segment rows (which cascades to tags + scan_runs).
/// Returns a per-file report so the UI can surface any paths that
/// couldn't be trashed (drive disconnected, permission denied, etc.).
#[tauri::command]
pub async fn delete_segments_to_trash(
    segment_ids: Vec<String>,
    in_memory_paths: HashMap<String, Vec<String>>,
    db: State<'_, DbHandle>,
) -> Result<DeleteReport, AppError> {
    let mut report = DeleteReport::default();

    // Resolve each segment's set of file paths. We accept an
    // `in_memory_paths` map from the frontend (it knows the full
    // channel list from the in-memory trips) and fall back to the
    // DB's stored master_path when a segment isn't in that map.
    let mut resolved: Vec<(String, Vec<String>)> = Vec::new();
    {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        for seg_id in &segment_ids {
            if let Some(paths) = in_memory_paths.get(seg_id) {
                resolved.push((seg_id.clone(), paths.clone()));
                continue;
            }
            let master: Option<String> = conn
                .query_row(
                    "SELECT master_path FROM segments WHERE id = ?1",
                    rusqlite::params![seg_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(p) = master {
                resolved.push((seg_id.clone(), vec![p]));
            }
        }
    }

    // Phase 1: trash the files. Collect per-file outcomes so we can
    // skip DB cleanup for segments where nothing could be removed.
    let mut any_success_per_segment: HashMap<String, bool> = HashMap::new();
    for (seg_id, paths) in &resolved {
        let mut any_ok = false;
        for path_str in paths {
            let path = std::path::Path::new(path_str);
            if !path.exists() {
                // Treat a missing file as already-gone; let DB cleanup
                // remove the stale segment row.
                any_ok = true;
                continue;
            }
            match trash::delete(path) {
                Ok(_) => {
                    report.files_trashed += 1;
                    any_ok = true;
                }
                Err(e) => {
                    report.failures.push(DeleteFailure {
                        path: path_str.clone(),
                        message: e.to_string(),
                    });
                }
            }
        }
        any_success_per_segment.insert(seg_id.clone(), any_ok);
    }

    // Phase 2: update the DB. For each segment whose files were
    // successfully trashed, we either:
    //   (a) hard-delete the row, when the trip has no completed
    //       timelapse to play back across the deleted span; or
    //   (b) tombstone the row (`is_tombstone = 1`, master_path = '',
    //       size_bytes = NULL), preserving the time topology so the
    //       timeline can render a hatched gap and the player can
    //       auto-switch to a tier across the deleted range.
    //
    // After tombstoning, if a trip has zero surviving non-tombstone
    // segments AND a completed timelapse, we hard-delete its tombstones
    // so the trip flips cleanly to archive-only via the existing
    // `list_archive_only_trips` path (which keys on "no segment rows").
    // Tags and scan_runs always cascade via explicit delete because
    // sqlite FKs aren't declared on these tables.
    {
        let mut conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let tx = conn.transaction()?;

        // Trips touched by this delete — we'll re-check each at the
        // end to decide whether to flush remaining tombstones.
        let mut touched_trips: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (seg_id, any_ok) in &any_success_per_segment {
            if !any_ok {
                continue;
            }

            // Look up the trip and decide tombstone vs. hard-delete.
            // A row may already be missing if the user issued a delete
            // for a stale segment id; treat that as "nothing to do".
            let row: Option<(String,)> = tx
                .query_row(
                    "SELECT trip_id FROM segments WHERE id = ?1",
                    rusqlite::params![seg_id],
                    |r| Ok((r.get::<_, String>(0)?,)),
                )
                .ok();
            let Some((trip_id,)) = row else { continue };

            let has_done_timelapse: bool = tx.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM timelapse_jobs
                    WHERE trip_id = ?1 AND status = ?2
                 )",
                rusqlite::params![&trip_id, crate::db::timelapse_jobs::STATUS_DONE],
                |r| r.get::<_, i64>(0),
            )? != 0;

            // Tags and scan_runs go either way — a tombstone has no
            // playable file to attach scan results or user tags to.
            tx.execute(
                "DELETE FROM tags WHERE segment_id = ?1",
                rusqlite::params![seg_id],
            )?;
            tx.execute(
                "DELETE FROM scan_runs WHERE segment_id = ?1",
                rusqlite::params![seg_id],
            )?;

            if has_done_timelapse {
                let n = tx.execute(
                    "UPDATE segments
                     SET is_tombstone = 1,
                         master_path = '',
                         size_bytes = NULL
                     WHERE id = ?1",
                    rusqlite::params![seg_id],
                )?;
                report.segments_removed += n;
                if n > 0 {
                    report.tombstoned_segment_ids.push(seg_id.clone());
                }
            } else {
                let n = tx.execute(
                    "DELETE FROM segments WHERE id = ?1",
                    rusqlite::params![seg_id],
                )?;
                report.segments_removed += n;
            }

            touched_trips.insert(trip_id);
        }

        // Per touched trip: if no surviving non-tombstone segments
        // remain, drop the tombstones so the trip flips to archive-only
        // (segment rows == 0). The timelapse_jobs guard on persist_and_gc
        // keeps the trip row alive on subsequent scans.
        for trip_id in &touched_trips {
            let surviving: i64 = tx.query_row(
                "SELECT COUNT(*) FROM segments
                 WHERE trip_id = ?1 AND is_tombstone = 0",
                rusqlite::params![trip_id],
                |r| r.get(0),
            )?;
            if surviving == 0 {
                tx.execute(
                    "DELETE FROM segments WHERE trip_id = ?1 AND is_tombstone = 1",
                    rusqlite::params![trip_id],
                )?;
                // The tombstones we just reported as such have been
                // hard-deleted under our feet; the frontend should
                // splice them out, not leave them in the timeline.
                report
                    .tombstoned_segment_ids
                    .retain(|id| {
                        // We don't know the seg→trip mapping here without
                        // a re-query, but it's small: any id whose row no
                        // longer exists has been folded into the archive.
                        let still_exists: i64 = tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM segments WHERE id = ?1)",
                                rusqlite::params![id],
                                |r| r.get(0),
                            )
                            .unwrap_or(0);
                        still_exists != 0
                    });
            }
        }

        tx.commit()?;
    }

    Ok(report)
}
