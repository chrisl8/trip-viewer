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

    // Phase 2: drop the DB rows for segments whose files are fully
    // (or newly) gone. Tags and scan_runs cascade via explicit delete
    // because sqlite FKs aren't declared on these tables.
    {
        let mut conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let tx = conn.transaction()?;
        for (seg_id, any_ok) in &any_success_per_segment {
            if !any_ok {
                continue;
            }
            tx.execute(
                "DELETE FROM tags WHERE segment_id = ?1",
                rusqlite::params![seg_id],
            )?;
            tx.execute(
                "DELETE FROM scan_runs WHERE segment_id = ?1",
                rusqlite::params![seg_id],
            )?;
            let n = tx.execute(
                "DELETE FROM segments WHERE id = ?1",
                rusqlite::params![seg_id],
            )?;
            report.segments_removed += n;
        }
        tx.commit()?;
    }

    Ok(report)
}
