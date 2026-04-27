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
    Ok(count)
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
