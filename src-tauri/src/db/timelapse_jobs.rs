//! CRUD for the `timelapse_jobs` table. Each row tracks one
//! (trip_id, tier, channel) encode: its status, output path, and the
//! ffmpeg/encoder it was produced with.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Primary status values for a job row.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseJobRow {
    pub trip_id: String,
    pub tier: String,
    pub channel: String,
    pub status: String,
    pub output_path: Option<String>,
    pub error_message: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub encoder_used: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    /// How many segments in the source concat were black-frame
    /// placeholders because the real sibling file was missing. 0 for
    /// clean runs. Non-zero means the output is valid and in sync but
    /// shows black frames on this channel for the affected segments.
    pub padded_count: i64,
    /// Piecewise speed curve for the (trip, tier). Serialized
    /// `Vec<speed_curve::CurveSegment>`. The frontend player uses this
    /// to map file-time ↔ concat-time for the timeline, map cursor,
    /// tags, and effective-speed math. Null for rows encoded before
    /// this column existed (user will rebuild-all).
    pub speed_curve_json: Option<String>,
}

/// Upsert a pending job row. If the row already exists in a completed
/// state, this resets it so a rebuild can proceed. Used when enqueueing
/// work; actual status transitions use `mark_*` helpers.
pub fn upsert_pending(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO timelapse_jobs
            (trip_id, tier, channel, status, created_at_ms, padded_count, speed_curve_json)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL)
         ON CONFLICT(trip_id, tier, channel) DO UPDATE SET
            status = excluded.status,
            output_path = NULL,
            error_message = NULL,
            completed_at_ms = NULL,
            padded_count = 0,
            speed_curve_json = NULL,
            created_at_ms = excluded.created_at_ms",
        params![trip_id, tier, channel, STATUS_PENDING, now],
    )?;
    Ok(())
}

pub fn mark_running(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE timelapse_jobs SET status = ?4
         WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
        params![trip_id, tier, channel, STATUS_RUNNING],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mark_done(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
    output_path: &str,
    ffmpeg_version: &str,
    encoder_used: &str,
    padded_count: i64,
    speed_curve_json: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE timelapse_jobs SET
            status = ?4,
            output_path = ?5,
            error_message = NULL,
            ffmpeg_version = ?6,
            encoder_used = ?7,
            completed_at_ms = ?8,
            padded_count = ?9,
            speed_curve_json = ?10
         WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
        params![
            trip_id,
            tier,
            channel,
            STATUS_DONE,
            output_path,
            ffmpeg_version,
            encoder_used,
            now,
            padded_count,
            speed_curve_json,
        ],
    )?;
    Ok(())
}

pub fn mark_failed(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
    error_message: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE timelapse_jobs SET
            status = ?4,
            error_message = ?5,
            completed_at_ms = ?6
         WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
        params![
            trip_id,
            tier,
            channel,
            STATUS_FAILED,
            error_message,
            now
        ],
    )?;
    Ok(())
}

/// Reset a row from 'running' back to 'pending'. Used when the user
/// cancels mid-encode and when startup cleanup finds stale 'running'
/// rows left behind by a hard process exit.
pub fn reset_to_pending(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE timelapse_jobs SET
            status = ?4,
            output_path = NULL,
            error_message = NULL,
            completed_at_ms = NULL,
            padded_count = 0,
            speed_curve_json = NULL
         WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
        params![trip_id, tier, channel, STATUS_PENDING],
    )?;
    Ok(())
}

pub fn get(
    conn: &Connection,
    trip_id: &str,
    tier: &str,
    channel: &str,
) -> Result<Option<TimelapseJobRow>, AppError> {
    let row = conn
        .query_row(
            "SELECT trip_id, tier, channel, status, output_path, error_message,
                    ffmpeg_version, encoder_used, created_at_ms, completed_at_ms,
                    padded_count, speed_curve_json
             FROM timelapse_jobs
             WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
            params![trip_id, tier, channel],
            row_to_job,
        )
        .optional()?;
    Ok(row)
}

pub fn list_all(conn: &Connection) -> Result<Vec<TimelapseJobRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT trip_id, tier, channel, status, output_path, error_message,
                ffmpeg_version, encoder_used, created_at_ms, completed_at_ms,
                padded_count, speed_curve_json
         FROM timelapse_jobs
         ORDER BY created_at_ms DESC",
    )?;
    let rows = stmt.query_map([], row_to_job)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn list_by_status(
    conn: &Connection,
    status: &str,
) -> Result<Vec<TimelapseJobRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT trip_id, tier, channel, status, output_path, error_message,
                ffmpeg_version, encoder_used, created_at_ms, completed_at_ms,
                padded_count, speed_curve_json
         FROM timelapse_jobs
         WHERE status = ?1
         ORDER BY created_at_ms ASC",
    )?;
    let rows = stmt.query_map(params![status], row_to_job)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn row_to_job(r: &rusqlite::Row) -> rusqlite::Result<TimelapseJobRow> {
    Ok(TimelapseJobRow {
        trip_id: r.get(0)?,
        tier: r.get(1)?,
        channel: r.get(2)?,
        status: r.get(3)?,
        output_path: r.get(4)?,
        error_message: r.get(5)?,
        ffmpeg_version: r.get(6)?,
        encoder_used: r.get(7)?,
        created_at_ms: r.get(8)?,
        completed_at_ms: r.get(9)?,
        padded_count: r.get(10)?,
        speed_curve_json: r.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn upsert_creates_pending_row() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "trip-1", "8x", "F").unwrap();
        let row = get(&conn, "trip-1", "8x", "F").unwrap().unwrap();
        assert_eq!(row.status, STATUS_PENDING);
        assert_eq!(row.trip_id, "trip-1");
        assert!(row.output_path.is_none());
    }

    #[test]
    fn status_transitions() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "16x", "I").unwrap();
        mark_running(&conn, "t", "16x", "I").unwrap();
        assert_eq!(get(&conn, "t", "16x", "I").unwrap().unwrap().status, STATUS_RUNNING);
        mark_done(&conn, "t", "16x", "I", "/out/f.mp4", "7.0", "hevc_nvenc", 0, "[]").unwrap();
        let row = get(&conn, "t", "16x", "I").unwrap().unwrap();
        assert_eq!(row.status, STATUS_DONE);
        assert_eq!(row.output_path.as_deref(), Some("/out/f.mp4"));
        assert_eq!(row.encoder_used.as_deref(), Some("hevc_nvenc"));
    }

    #[test]
    fn reset_clears_output() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "8x", "F").unwrap();
        mark_running(&conn, "t", "8x", "F").unwrap();
        reset_to_pending(&conn, "t", "8x", "F").unwrap();
        let row = get(&conn, "t", "8x", "F").unwrap().unwrap();
        assert_eq!(row.status, STATUS_PENDING);
        assert!(row.output_path.is_none());
    }

    #[test]
    fn list_by_status_filters() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "a", "8x", "F").unwrap();
        upsert_pending(&conn, "b", "8x", "F").unwrap();
        mark_running(&conn, "a", "8x", "F").unwrap();
        assert_eq!(list_by_status(&conn, STATUS_PENDING).unwrap().len(), 1);
        assert_eq!(list_by_status(&conn, STATUS_RUNNING).unwrap().len(), 1);
    }

    #[test]
    fn upsert_resets_a_done_row() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "8x", "F").unwrap();
        mark_running(&conn, "t", "8x", "F").unwrap();
        mark_done(&conn, "t", "8x", "F", "/x.mp4", "7.0", "hevc_nvenc", 0, "[]").unwrap();
        upsert_pending(&conn, "t", "8x", "F").unwrap();
        let row = get(&conn, "t", "8x", "F").unwrap().unwrap();
        assert_eq!(row.status, STATUS_PENDING);
        assert!(row.output_path.is_none());
        assert_eq!(row.padded_count, 0);
        assert!(row.speed_curve_json.is_none());
    }

    #[test]
    fn padded_count_round_trips() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "16x", "R").unwrap();
        mark_running(&conn, "t", "16x", "R").unwrap();
        mark_done(
            &conn,
            "t",
            "16x",
            "R",
            "/x.mp4",
            "7.0",
            "hevc_nvenc",
            3,
            r#"[{"concatStart":0,"concatEnd":60,"rate":16}]"#,
        )
        .unwrap();
        let row = get(&conn, "t", "16x", "R").unwrap().unwrap();
        assert_eq!(row.padded_count, 3);
    }

    #[test]
    fn reset_clears_padded_count_and_curve() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "16x", "R").unwrap();
        mark_done(
            &conn,
            "t",
            "16x",
            "R",
            "/x.mp4",
            "7.0",
            "hevc_nvenc",
            2,
            r#"[{"concatStart":0,"concatEnd":60,"rate":16}]"#,
        )
        .unwrap();
        reset_to_pending(&conn, "t", "16x", "R").unwrap();
        let row = get(&conn, "t", "16x", "R").unwrap().unwrap();
        assert_eq!(row.padded_count, 0);
        assert!(row.speed_curve_json.is_none());
    }

    #[test]
    fn speed_curve_json_round_trips() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        upsert_pending(&conn, "t", "16x", "F").unwrap();
        let curve =
            r#"[{"concatStart":0,"concatEnd":25,"rate":16},{"concatStart":25,"concatEnd":40,"rate":1},{"concatStart":40,"concatEnd":60,"rate":16}]"#;
        mark_done(
            &conn, "t", "16x", "F", "/x.mp4", "7.0", "hevc_nvenc", 0, curve,
        )
        .unwrap();
        let row = get(&conn, "t", "16x", "F").unwrap().unwrap();
        assert_eq!(row.speed_curve_json.as_deref(), Some(curve));
    }
}
