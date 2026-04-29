//! Stale-job recovery. Called once at app startup after DB migrations.
//!
//! If the app was killed (hard exit, crash, power loss) while a trip
//! was encoding, the child ffmpeg process ends too and leaves behind
//! a partial .mp4 with no finalized moov atom — unplayable garbage.
//! The `timelapse_jobs` row is still marked `running`.
//!
//! On next launch we find every `running` row, delete any partial
//! output file, and reset the row to `pending` so it picks up again
//! on the next start.

use std::fs;

use rusqlite::params;

use crate::db::{self, DbHandle};
use crate::error::AppError;

pub fn cleanup_stale_jobs(db: &DbHandle) -> Result<u64, AppError> {
    let running = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        db::timelapse_jobs::list_by_status(&conn, db::timelapse_jobs::STATUS_RUNNING)?
    };
    let count = running.len() as u64;
    for row in running {
        if let Some(path) = row.output_path.as_deref() {
            if let Err(e) = fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "[timelapse] cleanup: could not remove {path}: {e} (continuing)"
                    );
                }
            }
        }
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        db::timelapse_jobs::reset_to_pending(
            &conn,
            &row.trip_id,
            &row.tier,
            &row.channel,
        )?;
    }
    if count > 0 {
        eprintln!("[timelapse] cleanup: reset {count} stale running job(s) to pending");
    }
    backfill_output_sizes(db)?;
    Ok(count)
}

/// One-shot pass to fill `output_size_bytes` on done rows that were
/// completed before migration 0009 (or whose output was missing at
/// completion time and is now present). Cheap — one stat per
/// completed job, dozens per typical library.
fn backfill_output_sizes(db: &DbHandle) -> Result<(), AppError> {
    let rows: Vec<(String, String, String, String)> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT trip_id, tier, channel, output_path
             FROM timelapse_jobs
             WHERE status = ?1
               AND output_path IS NOT NULL
               AND output_size_bytes IS NULL",
        )?;
        let mapped = stmt.query_map(params![db::timelapse_jobs::STATUS_DONE], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in mapped {
            out.push(r?);
        }
        out
    };
    if rows.is_empty() {
        return Ok(());
    }
    let mut filled = 0u64;
    for (trip_id, tier, channel, output_path) in rows {
        let Ok(meta) = fs::metadata(&output_path) else {
            continue;
        };
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        conn.execute(
            "UPDATE timelapse_jobs SET output_size_bytes = ?4
             WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
            params![trip_id, tier, channel, meta.len() as i64],
        )?;
        filled += 1;
    }
    if filled > 0 {
        eprintln!("[timelapse] cleanup: backfilled output_size_bytes for {filled} completed job(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use std::env::temp_dir;
    use std::io::Write;

    #[test]
    fn resets_running_rows_and_deletes_output_files() {
        let db = open_in_memory().unwrap();

        // Create a partial output file on disk we can verify is deleted.
        let tmp = temp_dir().join("tripviewer-cleanup-test.mp4");
        {
            let mut f = fs::File::create(&tmp).unwrap();
            writeln!(f, "not a real mp4").unwrap();
        }

        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, "trip-1", "8x", "F").unwrap();
            db::timelapse_jobs::mark_running(&conn, "trip-1", "8x", "F").unwrap();
            // Simulate mid-encode state: output_path populated but status still running.
            conn.execute(
                "UPDATE timelapse_jobs SET output_path = ?1 WHERE trip_id = ?2",
                rusqlite::params![tmp.to_string_lossy().to_string(), "trip-1"],
            )
            .unwrap();
        }

        let reset_count = cleanup_stale_jobs(&db).unwrap();
        assert_eq!(reset_count, 1);
        assert!(!tmp.exists(), "partial output file should be removed");

        let conn = db.lock().unwrap();
        let row = db::timelapse_jobs::get(&conn, "trip-1", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, db::timelapse_jobs::STATUS_PENDING);
        assert!(row.output_path.is_none());
    }

    #[test]
    fn missing_output_file_is_not_an_error() {
        let db = open_in_memory().unwrap();
        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, "t", "8x", "F").unwrap();
            db::timelapse_jobs::mark_running(&conn, "t", "8x", "F").unwrap();
            conn.execute(
                "UPDATE timelapse_jobs SET output_path = ?1 WHERE trip_id = ?2",
                rusqlite::params!["C:/does/not/exist.mp4", "t"],
            )
            .unwrap();
        }
        let n = cleanup_stale_jobs(&db).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn noop_when_no_running_rows() {
        let db = open_in_memory().unwrap();
        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, "t", "8x", "F").unwrap();
        }
        let n = cleanup_stale_jobs(&db).unwrap();
        assert_eq!(n, 0);
    }
}
