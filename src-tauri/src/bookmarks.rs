//! Millisecond bookmarks on the concatenated trip clock, stored per archive.
use crate::archive::{require_db, ArchiveSlot};
use crate::error::AppError;
use rusqlite::{params, Connection};
use tauri::State;

fn list(conn: &Connection, trip_id: &str) -> Result<Vec<i64>, AppError> {
    let mut stmt =
        conn.prepare("SELECT time_ms FROM trip_bookmarks WHERE trip_id = ?1 ORDER BY time_ms")?;
    let rows = stmt.query_map([trip_id], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn set(conn: &Connection, trip_id: &str, time_ms: i64, present: bool) -> Result<(), AppError> {
    if time_ms < 0 {
        return Err(AppError::Internal(
            "bookmark time must be non-negative".into(),
        ));
    }
    if present {
        conn.execute(
            "INSERT OR IGNORE INTO trip_bookmarks (trip_id, time_ms) VALUES (?1, ?2)",
            params![trip_id, time_ms],
        )?;
    } else {
        conn.execute(
            "DELETE FROM trip_bookmarks WHERE trip_id = ?1 AND time_ms = ?2",
            params![trip_id, time_ms],
        )?;
    }
    Ok(())
}

/// Rebase bookmarks before segments are reassigned by a manual trip merge.
/// Segment-local offsets preserve the exact recorded moment even if trips overlap.
pub(crate) fn rebase_for_merge(
    conn: &Connection,
    primary: &str,
    absorbed: &[String],
) -> Result<(), AppError> {
    let mut ids = vec![primary.to_owned()];
    ids.extend_from_slice(absorbed);
    let saved = ids
        .iter()
        .map(|id| Ok((id.clone(), list(conn, id)?)))
        .collect::<Result<Vec<_>, AppError>>()?;
    if saved.iter().all(|(_, times)| times.is_empty()) {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT trip_id, duration_s, start_time_ms, id FROM segments ORDER BY start_time_ms, id",
    )?;
    let mut segments = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Archive-only trips contribute a virtual segment on their original clock.
    for id in &ids {
        if segments.iter().any(|(trip, _, _, _)| trip == id) {
            continue;
        }
        let (start, end): (i64, i64) = conn.query_row(
            "SELECT start_time_ms, end_time_ms FROM trips WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut curves = conn.prepare("SELECT speed_curve_json FROM timelapse_jobs WHERE trip_id=?1 AND status='done' AND speed_curve_json IS NOT NULL")?;
        let jsons = curves
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let duration = jsons
            .iter()
            .filter_map(|json| crate::timelapse::speed_curve::deserialize_curve(json))
            .flatten()
            .map(|c| c.concat_end)
            .filter(|s| s.is_finite() && *s > 0.0)
            .fold(0.0, f64::max);
        segments.push((
            id.clone(),
            if duration > 0.0 {
                duration
            } else {
                (end - start).max(0) as f64 / 1000.0
            },
            start,
            id.clone(),
        ));
    }
    segments.sort_by(|a, b| (a.2, &a.3).cmp(&(b.2, &b.3)));
    let mut local = std::collections::HashMap::<String, i64>::new();
    let mut mappings = std::collections::HashMap::<String, Vec<(i64, i64, i64)>>::new();
    let mut cursor = 0;
    for (id, duration, _, _) in segments
        .into_iter()
        .filter(|(id, _, _, _)| ids.contains(id))
    {
        let start = *local.get(&id).unwrap_or(&0);
        let duration = (duration * 1000.0).round() as i64;
        mappings
            .entry(id.clone())
            .or_default()
            .push((start, start + duration, cursor));
        local.insert(id, start + duration);
        cursor += duration;
    }
    let mut rebased = Vec::new();
    for (id, times) in saved {
        for time in times {
            let mapped = mappings
                .get(&id)
                .and_then(|runs| {
                    runs.iter()
                        .find(|(_, end, _)| time < *end)
                        .or_else(|| runs.last())
                })
                .map(|(start, _, target)| target + time - start)
                .unwrap_or(time);
            rebased.push(mapped);
        }
        conn.execute("DELETE FROM trip_bookmarks WHERE trip_id = ?1", [id])?;
    }
    for time in rebased {
        set(conn, primary, time, true)?;
    }
    Ok(())
}

fn checked_db(slot: &ArchiveSlot, archive_root: &str) -> Result<crate::db::DbHandle, AppError> {
    let db = require_db(slot)?;
    if db.archive_root() != std::path::Path::new(archive_root) {
        return Err(AppError::Internal(
            "archive changed; reload bookmarks".into(),
        ));
    }
    Ok(db)
}

#[tauri::command]
pub async fn list_bookmarks(
    trip_id: String,
    archive_root: String,
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<i64>, AppError> {
    let db = checked_db(&slot, &archive_root)?;
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    list(&conn, &trip_id)
}

#[tauri::command]
pub async fn set_bookmark(
    trip_id: String,
    archive_root: String,
    time_ms: i64,
    present: bool,
    slot: State<'_, ArchiveSlot>,
) -> Result<(), AppError> {
    let db = checked_db(&slot, &archive_root)?;
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    set(&conn, &trip_id, time_ms, present)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_sorted_isolated_idempotent_and_cascading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let db = crate::db::open_at_path(&path, dir.path()).unwrap();
            let conn = db.lock().unwrap();
            for id in ["a", "b"] {
                conn.execute("INSERT INTO trips(id,start_time_ms,end_time_ms,last_seen_ms) VALUES (?1,0,60000,0)", [id]).unwrap();
            }
            for time in [2000, 0, 1000, 1000] {
                set(&conn, "a", time, true).unwrap();
            }
            set(&conn, "b", 3000, true).unwrap();
            assert!(set(&conn, "a", -1, true).is_err());
            assert_eq!(list(&conn, "a").unwrap(), vec![0, 1000, 2000]);
        }
        let db = crate::db::open_at_path(&path, dir.path()).unwrap();
        let conn = db.lock().unwrap();
        assert_eq!(list(&conn, "a").unwrap(), vec![0, 1000, 2000]);
        set(&conn, "a", 1000, false).unwrap();
        assert_eq!(list(&conn, "a").unwrap(), vec![0, 2000]);
        assert_eq!(list(&conn, "b").unwrap(), vec![3000]);
        conn.execute("DELETE FROM trips WHERE id='a'", []).unwrap();
        assert!(list(&conn, "a").unwrap().is_empty());
    }
    #[test]
    fn merge_rebases_positions_and_id_rename_cascades() {
        let db = crate::db::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        for (id, start) in [("a", 0), ("b", 4000)] {
            conn.execute("INSERT INTO trips(id,start_time_ms,end_time_ms,last_seen_ms) VALUES (?1,?2,?2+4000,0)", params![id,start]).unwrap();
            conn.execute("INSERT INTO segments(id,trip_id,start_time_ms,duration_s,master_path,last_seen_ms) VALUES (?1,?1,?2,4,'clip',0)", params![id,start]).unwrap();
            set(&conn, id, 1500, true).unwrap();
        }
        rebase_for_merge(&conn, "a", &["b".to_owned()]).unwrap();
        assert_eq!(list(&conn, "a").unwrap(), vec![1500, 5500]);
        assert!(list(&conn, "b").unwrap().is_empty());
        conn.execute("UPDATE trips SET id='renamed' WHERE id='a'", [])
            .unwrap();
        assert_eq!(list(&conn, "renamed").unwrap(), vec![1500, 5500]);
    }
    #[test]
    fn archive_only_merge_uses_original_curve_clock_instead_of_wall_clock_gaps() {
        let db = crate::db::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        for (id, start) in [("a", 0), ("b", 120000)] {
            conn.execute("INSERT INTO trips(id,start_time_ms,end_time_ms,last_seen_ms) VALUES (?1,?2,?2+10000,0)", params![id,start]).unwrap();
            conn.execute(r#"INSERT INTO timelapse_jobs(trip_id,tier,channel,status,created_at_ms,speed_curve_json) VALUES (?1,'8x','F','done',0,'[{"concatStart":0,"concatEnd":4,"rate":8}]')"#, [id]).unwrap();
            set(&conn, id, 1500, true).unwrap();
        }
        rebase_for_merge(&conn, "a", &["b".to_owned()]).unwrap();
        assert_eq!(list(&conn, "a").unwrap(), vec![1500, 5500]);
    }
    #[test]
    fn stale_archive_cannot_read_or_write_another_archive() {
        let slot = crate::archive::new_slot();
        let db = crate::db::open_in_memory().unwrap();
        *slot.write().unwrap() = Some(db);
        assert!(checked_db(&slot, "/different-archive").is_err());
    }
}
