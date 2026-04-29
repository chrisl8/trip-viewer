//! CRUD for the `manual_trip_merges` table. Each row records a directive
//! "natural trip with id `absorbed_trip_id` should be folded into the
//! merged trip with id `primary_trip_id`." Used by `db::segments::
//! persist_and_gc` after natural grouping to relabel groups so user
//! merges survive a folder rescan.
//!
//! Trip IDs in this table are stored as their hex-string form to match
//! how the rest of the schema represents UUIDs.
//!
//! Loops are not allowed (a primary cannot also be absorbed elsewhere).
//! `insert_merge` enforces this; the apply step in `persist_and_gc` walks
//! the map non-recursively so any accidentally-inserted loop just
//! relabels once and stops, but it would still be a bug.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::error::AppError;

/// Insert a merge directive. The absorbed trip will be folded into the
/// primary on the next `persist_and_gc`. Errors if the absorbed trip is
/// already a primary (would create a loop) or if the absorbed and
/// primary IDs are equal.
pub fn insert_merge(
    conn: &Connection,
    primary: Uuid,
    absorbed: Uuid,
    created_ms: i64,
) -> Result<(), AppError> {
    if primary == absorbed {
        return Err(AppError::Internal(
            "cannot merge a trip into itself".into(),
        ));
    }
    // Reject loops: the absorbed trip must not already be a primary
    // for some other merge.
    let absorbed_is_primary: i64 = conn.query_row(
        "SELECT COUNT(*) FROM manual_trip_merges WHERE primary_trip_id = ?1",
        params![absorbed.to_string()],
        |r| r.get(0),
    )?;
    if absorbed_is_primary > 0 {
        return Err(AppError::Internal(format!(
            "cannot absorb trip {absorbed} — it is already the primary of another merge"
        )));
    }
    conn.execute(
        "INSERT INTO manual_trip_merges (absorbed_trip_id, primary_trip_id, created_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(absorbed_trip_id) DO UPDATE SET
            primary_trip_id = excluded.primary_trip_id,
            created_ms = excluded.created_ms",
        params![absorbed.to_string(), primary.to_string(), created_ms],
    )?;
    Ok(())
}

/// Remove a merge directive. The absorbed trip will reappear as its
/// natural self on the next `persist_and_gc`. No-op if absent.
#[allow(dead_code)]
pub fn delete_merge(conn: &Connection, absorbed: Uuid) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM manual_trip_merges WHERE absorbed_trip_id = ?1",
        params![absorbed.to_string()],
    )?;
    Ok(())
}

/// Map from absorbed trip ID to its primary. Used by the grouping
/// rewrite step. Empty if no merges have been recorded.
pub fn list_merges(conn: &Connection) -> Result<HashMap<String, String>, AppError> {
    let mut stmt =
        conn.prepare("SELECT absorbed_trip_id, primary_trip_id FROM manual_trip_merges")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (absorbed, primary) = row?;
        out.insert(absorbed, primary);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn uuid(byte: u8) -> Uuid {
        Uuid::from_bytes([byte; 16])
    }

    #[test]
    fn insert_and_list_roundtrip() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let primary = uuid(0xAA);
        let absorbed = uuid(0xBB);
        insert_merge(&conn, primary, absorbed, 1000).unwrap();
        let map = list_merges(&conn).unwrap();
        assert_eq!(map.get(&absorbed.to_string()), Some(&primary.to_string()));
    }

    #[test]
    fn insert_self_fails() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let id = uuid(0xCC);
        let err = insert_merge(&conn, id, id, 1000).unwrap_err();
        assert!(format!("{err}").contains("itself"));
    }

    #[test]
    fn insert_loop_fails() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let a = uuid(0x01);
        let b = uuid(0x02);
        let c = uuid(0x03);
        // a absorbs b — fine.
        insert_merge(&conn, a, b, 1000).unwrap();
        // Now try to absorb a (which is a primary) into c — should fail.
        let err = insert_merge(&conn, c, a, 1001).unwrap_err();
        assert!(format!("{err}").contains("already the primary"));
    }

    #[test]
    fn delete_removes_directive() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let primary = uuid(0xAA);
        let absorbed = uuid(0xBB);
        insert_merge(&conn, primary, absorbed, 1000).unwrap();
        delete_merge(&conn, absorbed).unwrap();
        assert!(list_merges(&conn).unwrap().is_empty());
    }

    #[test]
    fn upsert_overwrites_primary() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let primary_a = uuid(0xA1);
        let primary_b = uuid(0xA2);
        let absorbed = uuid(0xBB);
        insert_merge(&conn, primary_a, absorbed, 1000).unwrap();
        insert_merge(&conn, primary_b, absorbed, 1100).unwrap();
        let map = list_merges(&conn).unwrap();
        assert_eq!(
            map.get(&absorbed.to_string()),
            Some(&primary_b.to_string()),
        );
    }
}
