//! Durable, independently reusable GPS analysis for completed segments.
use crate::error::AppError;
use rusqlite::{params, Connection, OptionalExtension};

pub fn load(
    conn: &Connection,
    trip_id: &str,
    segment_id: &str,
    signature: &str,
    parser_version: i32,
) -> Result<Option<String>, AppError> {
    Ok(conn
        .query_row(
            "SELECT payload_json FROM gps_segment_checkpoints
         WHERE trip_id = ?1 AND segment_id = ?2 AND source_signature = ?3 AND parser_version = ?4",
            params![trip_id, segment_id, signature, parser_version],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn upsert(
    conn: &Connection,
    trip_id: &str,
    segment_id: &str,
    signature: &str,
    parser_version: i32,
    payload: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO gps_segment_checkpoints
         (trip_id, segment_id, source_signature, parser_version, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(trip_id, segment_id) DO UPDATE SET
         source_signature = excluded.source_signature,
         parser_version = excluded.parser_version, payload_json = excluded.payload_json",
        params![trip_id, segment_id, signature, parser_version, payload],
    )?;
    Ok(())
}
