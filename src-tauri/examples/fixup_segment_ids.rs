//! One-off: walk the target DB's segments and trips, ensure every
//! `id` is consistent with `derive(master_path, start_ms)` /
//! `derive_trip_id(first_segment_id)`. Cascade FK changes.
//!
//! Found 67 inconsistencies in the user's test archive after the
//! cross-OS rebuild — root cause not yet identified, but this brings
//! the DB back to a consistent state so the legacy import can match
//! UUIDs cleanly.
//!
//! Usage: cargo run --example fixup_segment_ids -- '/path/to/archive'

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::DateTime;
use rusqlite::{params, Connection};
use uuid::Uuid;

const NS: Uuid = Uuid::from_bytes([
    0x5d, 0x11, 0x77, 0x2f, 0x8e, 0x3a, 0x4c, 0x22,
    0xa1, 0x9d, 0xf5, 0x6c, 0x3b, 0x8d, 0x7a, 0x4e,
]);

fn derive_segment_id(rel: &str, ms: i64) -> Uuid {
    let start = DateTime::from_timestamp_millis(ms).unwrap().naive_utc();
    let key = format!("seg|{}|{}", rel, start.and_utc().timestamp_millis());
    Uuid::new_v5(&NS, key.as_bytes())
}

fn derive_trip_id(first_segment_id: Uuid) -> Uuid {
    Uuid::new_v5(&NS, first_segment_id.as_bytes())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let archive = std::env::args().nth(1).expect("usage: fixup_segment_ids <archive_path>");
    let db_path = PathBuf::from(&archive)
        .join(".tripviewer")
        .join("tripviewer.db");
    let conn = Connection::open(&db_path)?;

    // Phase 1: segment id ≡ derive(master_path, start_ms)
    let mut seg_remap: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, master_path, start_time_ms FROM segments
             WHERE is_tombstone = 0 AND master_path != ''",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for r in rows {
            let (id, path, ms) = r?;
            let want = derive_segment_id(&path, ms).to_string();
            if want != id {
                seg_remap.insert(id, want);
            }
        }
    }
    println!("segments to fix: {}", seg_remap.len());

    // Phase 2: build new trip mapping based on first-segment IDs after
    // the segment fix. We compute trip_id from the *new* first-segment
    // id (post-remap).
    let mut trip_remap: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT t.id,
                    (SELECT s.id FROM segments s
                     WHERE s.trip_id = t.id
                     ORDER BY s.start_time_ms LIMIT 1) AS first_seg_id
             FROM trips t",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for r in rows {
            let (trip_id, first_seg) = r?;
            let Some(legacy_first_seg) = first_seg else { continue };
            let new_first_seg = seg_remap
                .get(&legacy_first_seg)
                .cloned()
                .unwrap_or(legacy_first_seg);
            let new_first_uuid = Uuid::parse_str(&new_first_seg)?;
            let want_trip = derive_trip_id(new_first_uuid).to_string();
            if want_trip != trip_id {
                trip_remap.insert(trip_id, want_trip);
            }
        }
    }
    println!("trips to fix: {}", trip_remap.len());

    let tx = conn.unchecked_transaction()?;

    // Cascade segment-id changes through FK columns first (so old IDs
    // still resolve in segments).
    for (old_id, new_id) in &seg_remap {
        tx.execute(
            "UPDATE tags SET segment_id = ?1 WHERE segment_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE scan_runs SET segment_id = ?1 WHERE segment_id = ?2",
            params![new_id, old_id],
        )?;
    }
    // Cascade trip-id changes.
    for (old_id, new_id) in &trip_remap {
        tx.execute(
            "UPDATE tags SET trip_id = ?1 WHERE trip_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE timelapse_jobs SET trip_id = ?1 WHERE trip_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE manual_trip_merges SET primary_trip_id = ?1 WHERE primary_trip_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE manual_trip_merges SET absorbed_trip_id = ?1 WHERE absorbed_trip_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE segments SET trip_id = ?1 WHERE trip_id = ?2",
            params![new_id, old_id],
        )?;
    }
    // Now fix segment.id and trip.id themselves.
    for (old_id, new_id) in &seg_remap {
        tx.execute(
            "UPDATE segments SET id = ?1 WHERE id = ?2",
            params![new_id, old_id],
        )?;
    }
    for (old_id, new_id) in &trip_remap {
        tx.execute(
            "UPDATE trips SET id = ?1 WHERE id = ?2",
            params![new_id, old_id],
        )?;
    }
    tx.commit()?;

    println!("done.");
    Ok(())
}
