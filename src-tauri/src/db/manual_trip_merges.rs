//! CRUD for the `manual_trip_merges` table. Each row records a directive
//! "natural trip with id `absorbed_trip_id` should be folded into the
//! merged trip with id `primary_trip_id`." Used by `db::segments::
//! persist_and_gc` after natural grouping to relabel groups so user
//! merges survive a folder rescan.
//!
//! Trip IDs in this table are stored as their hex-string form to match
//! how the rest of the schema represents UUIDs.
//!
//! Invariant: the table holds at most one level of indirection. A
//! trip is either a primary or an absorbed or neither, but never
//! both at once. `insert_merge` enforces this by flattening incoming
//! chains rather than rejecting them — the apply step in
//! `persist_and_gc` walks the map non-recursively, so a multi-hop
//! chain `C → B → A` wouldn't fully collapse without this guarantee.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::AppError;

/// Insert a merge directive. The absorbed trip will be folded into the
/// primary on the next `persist_and_gc`.
///
/// Both directions of chain are flattened on insert so the table stays
/// one level deep:
///   1. If the requested `primary` is itself absorbed by some root R,
///      the new row is rewritten to use R as the primary directly.
///   2. If the requested `absorbed` is already a primary for other
///      trips, those trips' rows are redirected to point at the
///      resolved root, so the previously-absorbed trips inherit the
///      new merge.
///
/// Errors if the absorbed and primary IDs are equal (either before or
/// after resolving the primary to its root) or if a cycle is detected
/// while walking the chain (shouldn't happen given this invariant, but
/// we defend against it).
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
    // If `primary` is itself already absorbed somewhere, resolve to
    // the ultimate root. Otherwise we'd violate the one-level
    // invariant and the apply step would only walk one hop.
    let resolved_primary = resolve_to_root(conn, primary)?;
    if resolved_primary == absorbed {
        // After resolution the user's request collapses to "merge a
        // trip into itself" — happens if they pick a trip that's
        // already been absorbed into the absorbed candidate's chain.
        return Err(AppError::Internal(
            "cannot merge a trip into itself (after resolving primary chain)".into(),
        ));
    }
    // If `absorbed` already has trips merged INTO it, redirect those
    // rows to point at the resolved primary so the previously-
    // absorbed trips come along for the ride.
    conn.execute(
        "UPDATE manual_trip_merges SET primary_trip_id = ?1
         WHERE primary_trip_id = ?2",
        params![resolved_primary.to_string(), absorbed.to_string()],
    )?;
    conn.execute(
        "INSERT INTO manual_trip_merges (absorbed_trip_id, primary_trip_id, created_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(absorbed_trip_id) DO UPDATE SET
            primary_trip_id = excluded.primary_trip_id,
            created_ms = excluded.created_ms",
        params![
            absorbed.to_string(),
            resolved_primary.to_string(),
            created_ms,
        ],
    )?;
    Ok(())
}

/// Follow the `absorbed → primary` chain from `start` until we hit a
/// trip that isn't absorbed by anyone. Returns `start` itself when
/// there's no existing row. Detects cycles (shouldn't be reachable
/// given the one-level invariant, but `manual_trip_merges` writes from
/// other paths could in theory create one).
fn resolve_to_root(conn: &Connection, start: Uuid) -> Result<Uuid, AppError> {
    let mut current = start;
    let mut seen: HashSet<Uuid> = HashSet::new();
    seen.insert(current);
    loop {
        let parent: Option<String> = conn
            .query_row(
                "SELECT primary_trip_id FROM manual_trip_merges
                 WHERE absorbed_trip_id = ?1",
                params![current.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(parent_str) = parent else {
            return Ok(current);
        };
        let parent_uuid = Uuid::parse_str(&parent_str).map_err(|e| {
            AppError::Internal(format!(
                "manual_trip_merges has malformed primary_trip_id {parent_str}: {e}"
            ))
        })?;
        if !seen.insert(parent_uuid) {
            return Err(AppError::Internal(format!(
                "cycle detected in manual_trip_merges chain starting at {start}"
            )));
        }
        current = parent_uuid;
    }
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

    /// Chain-flattening: `a absorbs b`, then `c absorbs a`. `a` was a
    /// primary; the new merge must redirect a's child (b) to c too,
    /// then record c as the new primary. End state: b → c and a → c,
    /// one level of indirection.
    #[test]
    fn inserting_when_absorbed_was_primary_flattens_chain() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let a = uuid(0x01);
        let b = uuid(0x02);
        let c = uuid(0x03);

        insert_merge(&conn, a, b, 1000).unwrap();
        // c now absorbs a, which was a primary for b. Previously this
        // returned "cannot absorb" — now it succeeds and flattens.
        insert_merge(&conn, c, a, 1001).unwrap();

        let map = list_merges(&conn).unwrap();
        // Both a and b should now point at c. No row should still
        // reference a as a primary.
        assert_eq!(map.get(&a.to_string()), Some(&c.to_string()));
        assert_eq!(map.get(&b.to_string()), Some(&c.to_string()));
        let a_as_primary: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manual_trip_merges WHERE primary_trip_id = ?1",
                params![a.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_as_primary, 0, "a must no longer appear as a primary");
    }

    /// Inverse chain-flattening: `a absorbs b`, then user picks `b`
    /// (which is already absorbed) as the primary of a new merge.
    /// `b` resolves to root `a`, so the new row is recorded with
    /// primary=a, not primary=b. Stays one level deep.
    #[test]
    fn inserting_when_primary_was_absorbed_resolves_to_root() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let a = uuid(0x01);
        let b = uuid(0x02);
        let c = uuid(0x03);

        insert_merge(&conn, a, b, 1000).unwrap();
        // b is already absorbed by a. The user nonetheless picks b
        // as the new primary for c. We should resolve b → a and
        // record c → a (not c → b).
        insert_merge(&conn, b, c, 1001).unwrap();

        let map = list_merges(&conn).unwrap();
        assert_eq!(map.get(&c.to_string()), Some(&a.to_string()));
        assert_eq!(map.get(&b.to_string()), Some(&a.to_string()));
    }

    /// Self-merge after resolving the primary chain: `a absorbs b`,
    /// then the user requests `b absorbs a`. b resolves to root a,
    /// which equals the absorbed argument — that's a self-merge in
    /// disguise. Must be rejected with a clear message.
    #[test]
    fn inserting_self_merge_after_resolution_fails() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let a = uuid(0x01);
        let b = uuid(0x02);

        insert_merge(&conn, a, b, 1000).unwrap();
        let err = insert_merge(&conn, b, a, 1001).unwrap_err();
        assert!(format!("{err}").contains("itself"));
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
