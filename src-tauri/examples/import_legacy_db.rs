//! One-off tool to import tags / scan_runs / timelapse_jobs / places /
//! manual_trip_merges from a legacy single-archive Trip Viewer DB
//! (typically a Windows backup) into the current per-archive DB on a
//! different OS.
//!
//! Skips segments and trips themselves — they should already exist in
//! the target with matching UUIDs after the cross-OS rewrite has
//! recomputed UUIDs from archive-relative paths. The tool computes
//! the same mapping locally to remap FK columns in the imported rows.
//!
//! Run with the **app closed** (SQLite supports concurrent reads but
//! not writes). Idempotency is not guaranteed — running twice
//! produces duplicate tags. Verify on a backup of the target DB.
//!
//! Usage:
//!   cargo run --example import_legacy_db -- \
//!     --legacy-db /path/to/legacy.db \
//!     --legacy-archive-root 'E:\Wolfbox Dashcam' \
//!     --target-archive '/run/media/chris10/Matrix/Wolfbox Dashcam'

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OpenFlags};
use uuid::Uuid;

// Mirror of TRIPVIEWER_ID_NS in src/model.rs. Inlined so this example
// doesn't depend on making that module pub.
const TRIPVIEWER_ID_NS: Uuid = Uuid::from_bytes([
    0x5d, 0x11, 0x77, 0x2f, 0x8e, 0x3a, 0x4c, 0x22,
    0xa1, 0x9d, 0xf5, 0x6c, 0x3b, 0x8d, 0x7a, 0x4e,
]);

fn derive_segment_id(rel_path: &str, start_time: NaiveDateTime) -> Uuid {
    let key = format!("seg|{}|{}", rel_path, start_time.and_utc().timestamp_millis());
    Uuid::new_v5(&TRIPVIEWER_ID_NS, key.as_bytes())
}

fn derive_trip_id(first_segment_id: Uuid) -> Uuid {
    Uuid::new_v5(&TRIPVIEWER_ID_NS, first_segment_id.as_bytes())
}

/// String-only relativization. Lowercase-equivalent on Windows drive
/// letters so `E:\foo` matches both `E:\Foo` and `e:\foo`.
fn relativize_str(abs: &str, prefix: &str) -> Option<String> {
    let abs = abs.replace('\\', "/");
    let prefix = prefix.replace('\\', "/");
    let prefix = prefix.trim_end_matches('/');
    if abs.len() < prefix.len() {
        return None;
    }
    let case_insensitive_drive = abs.starts_with(|c: char| c.is_ascii_alphabetic())
        && abs.as_bytes().get(1) == Some(&b':')
        && prefix.starts_with(|c: char| c.is_ascii_alphabetic())
        && prefix.as_bytes().get(1) == Some(&b':');
    let matches = if case_insensitive_drive {
        abs.as_bytes()
            .iter()
            .zip(prefix.as_bytes().iter())
            .take(prefix.len())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    } else {
        abs.starts_with(prefix)
    };
    if !matches {
        return None;
    }
    let stripped = abs[prefix.len()..].trim_start_matches('/');
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn parse_args() -> (String, String, String) {
    let mut legacy_db = None;
    let mut legacy_root = None;
    let mut target_archive = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--legacy-db" => legacy_db = args.next(),
            "--legacy-archive-root" => legacy_root = args.next(),
            "--target-archive" => target_archive = args.next(),
            _ => {
                eprintln!("unknown arg: {a}");
                std::process::exit(2);
            }
        }
    }
    let usage = "usage: import_legacy_db --legacy-db <path> --legacy-archive-root <str> --target-archive <path>";
    (
        legacy_db.unwrap_or_else(|| {
            eprintln!("{usage}");
            std::process::exit(2);
        }),
        legacy_root.unwrap_or_else(|| {
            eprintln!("{usage}");
            std::process::exit(2);
        }),
        target_archive.unwrap_or_else(|| {
            eprintln!("{usage}");
            std::process::exit(2);
        }),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (legacy_db_path, legacy_root, target_archive) = parse_args();

    println!("legacy DB:           {legacy_db_path}");
    println!("legacy archive root: {legacy_root}");
    println!("target archive:      {target_archive}");
    println!();

    // Open legacy DB read-only — no migrations applied. We only read
    // columns that exist at every recent schema version.
    let legacy = Connection::open_with_flags(
        &legacy_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;

    // Target DB. Open via direct rusqlite (the running app is assumed
    // to be closed; the cross-OS rewrite already brought this DB to
    // schema head 11).
    let target_root = PathBuf::from(&target_archive);
    let target_db_path = target_root.join(".tripviewer").join("tripviewer.db");
    let mut target = Connection::open(&target_db_path)?;
    target.pragma_update(None, "foreign_keys", "ON")?;

    // ── Build segment ID map ──────────────────────────────────────
    let mut seg_map: HashMap<String, String> = HashMap::new();
    let mut seg_skipped_outside = 0usize;
    {
        let mut stmt =
            legacy.prepare("SELECT id, master_path, start_time_ms FROM segments WHERE master_path != ''")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (legacy_id, abs_path, start_ms) = row?;
            let Some(rel) = relativize_str(&abs_path, &legacy_root) else {
                seg_skipped_outside += 1;
                continue;
            };
            let start_time = DateTime::<Utc>::from_timestamp_millis(start_ms)
                .ok_or_else(|| format!("bad start_time_ms {start_ms}"))?
                .naive_utc();
            let new_id = derive_segment_id(&rel, start_time).to_string();
            seg_map.insert(legacy_id, new_id);
        }
    }
    println!(
        "segments: mapped {} ({} skipped — outside legacy archive root)",
        seg_map.len(),
        seg_skipped_outside
    );

    // ── Build trip ID map ─────────────────────────────────────────
    let mut trip_map: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = legacy.prepare(
            "SELECT t.id,
                    (SELECT s.id FROM segments s
                     WHERE s.trip_id = t.id
                     ORDER BY s.start_time_ms LIMIT 1) AS first_seg_id
             FROM trips t",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (legacy_trip, first_seg) = row?;
            let Some(legacy_first) = first_seg else { continue };
            let Some(new_first) = seg_map.get(&legacy_first) else { continue };
            let new_first_uuid = Uuid::parse_str(new_first)?;
            let new_trip_id = derive_trip_id(new_first_uuid).to_string();
            trip_map.insert(legacy_trip, new_trip_id);
        }
    }
    println!("trips: mapped {}", trip_map.len());

    // ── Sanity check: target has matching segments ───────────────
    let target_seg_ids: HashSet<String> = {
        let mut stmt = target.prepare("SELECT id FROM segments")?;
        let mut out = HashSet::new();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            out.insert(r?);
        }
        out
    };
    let mapped_in_target = seg_map
        .values()
        .filter(|v| target_seg_ids.contains(*v))
        .count();
    println!(
        "target DB has {} segments; {} of our mapped IDs land in it",
        target_seg_ids.len(),
        mapped_in_target
    );

    let target_trip_ids: HashSet<String> = {
        let mut stmt = target.prepare("SELECT id FROM trips")?;
        let mut out = HashSet::new();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            out.insert(r?);
        }
        out
    };
    let mapped_trips_in_target = trip_map
        .values()
        .filter(|v| target_trip_ids.contains(*v))
        .count();
    println!(
        "target DB has {} trips; {} of our mapped trip IDs land in it",
        target_trip_ids.len(),
        mapped_trips_in_target
    );
    println!();

    let tx = target.transaction()?;

    // ── tags ─────────────────────────────────────────────────────
    let mut tags_inserted = 0usize;
    let mut tags_orphaned = 0usize;
    {
        let mut stmt = legacy.prepare(
            "SELECT segment_id, trip_id, name, category, source, scan_id, scan_version,
                    confidence, start_ms, end_ms, note, metadata_json, created_ms
             FROM tags",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<f64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<i64>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, i64>(12)?,
            ))
        })?;
        for row in rows {
            let (
                seg_id,
                trip_id,
                name,
                category,
                source,
                scan_id,
                scan_version,
                confidence,
                start_ms,
                end_ms,
                note,
                metadata_json,
                created_ms,
            ) = row?;
            let new_seg = seg_id
                .as_ref()
                .and_then(|s| seg_map.get(s).cloned())
                .filter(|id| target_seg_ids.contains(id));
            let new_trip = trip_id
                .as_ref()
                .and_then(|t| trip_map.get(t).cloned())
                .filter(|id| target_trip_ids.contains(id));
            if new_seg.is_none() && new_trip.is_none() {
                tags_orphaned += 1;
                continue;
            }
            tx.execute(
                "INSERT INTO tags (segment_id, trip_id, name, category, source, scan_id,
                                   scan_version, confidence, start_ms, end_ms, note,
                                   metadata_json, created_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    new_seg,
                    new_trip,
                    name,
                    category,
                    source,
                    scan_id,
                    scan_version,
                    confidence,
                    start_ms,
                    end_ms,
                    note,
                    metadata_json,
                    created_ms,
                ],
            )?;
            tags_inserted += 1;
        }
    }
    println!("tags: inserted {tags_inserted} ({tags_orphaned} skipped — orphan after remap)");

    // ── scan_runs ───────────────────────────────────────────────
    let mut scan_runs_inserted = 0usize;
    let mut scan_runs_skipped = 0usize;
    {
        let mut stmt = legacy.prepare(
            "SELECT segment_id, scan_id, version, status, ran_at_ms, error_message
             FROM scan_runs",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (legacy_seg, scan_id, version, status, ran_at_ms, error_message) = row?;
            let Some(new_seg) = seg_map.get(&legacy_seg).cloned() else {
                scan_runs_skipped += 1;
                continue;
            };
            if !target_seg_ids.contains(&new_seg) {
                scan_runs_skipped += 1;
                continue;
            }
            // INSERT OR IGNORE to skip duplicates if the legacy DB
            // happened to record the same (segment, scan) pair more
            // than once after our remap collapses them.
            tx.execute(
                "INSERT OR IGNORE INTO scan_runs (segment_id, scan_id, version, status, ran_at_ms, error_message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![new_seg, scan_id, version, status, ran_at_ms, error_message],
            )?;
            scan_runs_inserted += 1;
        }
    }
    println!("scan_runs: inserted {scan_runs_inserted} ({scan_runs_skipped} skipped)");

    // ── timelapse_jobs ──────────────────────────────────────────
    // output_path gets the Windows prefix swapped for the Linux
    // archive root. The .mp4 filename keeps its OLD trip_id —
    // those files exist on the user's drive under that name and
    // renaming them is a separate cleanup step. Trip rebuilds will
    // generate fresh files with the new trip_id naming.
    let mut tl_inserted = 0usize;
    let mut tl_skipped = 0usize;
    {
        let mut stmt = legacy.prepare(
            "SELECT trip_id, tier, channel, status, output_path, error_message,
                    created_at_ms, completed_at_ms, padded_count, speed_curve_json,
                    encoder_used, ffmpeg_version, output_size_bytes
             FROM timelapse_jobs",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<i64>>(12)?,
            ))
        })?;
        for row in rows {
            let (
                legacy_trip,
                tier,
                channel,
                status,
                output_path,
                error_message,
                created_at_ms,
                completed_at_ms,
                padded_count,
                speed_curve_json,
                encoder_used,
                ffmpeg_version,
                output_size_bytes,
            ) = row?;
            let Some(new_trip) = trip_map.get(&legacy_trip).cloned() else {
                tl_skipped += 1;
                continue;
            };
            if !target_trip_ids.contains(&new_trip) {
                tl_skipped += 1;
                continue;
            }
            // Rewrite output_path: strip legacy archive prefix, prepend
            // target archive root. Stays absolute (matches the rest of
            // the codebase's expectations for output_path).
            let new_output_path = output_path.as_ref().and_then(|p| {
                relativize_str(p, &legacy_root).map(|rel| {
                    let mut out = PathBuf::from(&target_archive);
                    out.push(&rel);
                    out.to_string_lossy().into_owned()
                })
            });
            tx.execute(
                "INSERT INTO timelapse_jobs (trip_id, tier, channel, status, output_path,
                                             error_message, created_at_ms, completed_at_ms,
                                             padded_count, speed_curve_json, encoder_used,
                                             ffmpeg_version, output_size_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    new_trip,
                    tier,
                    channel,
                    status,
                    new_output_path,
                    error_message,
                    created_at_ms,
                    completed_at_ms,
                    padded_count,
                    speed_curve_json,
                    encoder_used,
                    ffmpeg_version,
                    output_size_bytes,
                ],
            )?;
            tl_inserted += 1;
        }
    }
    println!("timelapse_jobs: inserted {tl_inserted} ({tl_skipped} skipped)");

    // ── places ──────────────────────────────────────────────────
    let mut places_inserted = 0usize;
    {
        let mut stmt = legacy.prepare(
            "SELECT name, lat, lon, radius_m, created_ms FROM places",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (name, lat, lon, radius_m, created_ms) = row?;
            tx.execute(
                "INSERT INTO places (name, lat, lon, radius_m, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, lat, lon, radius_m, created_ms],
            )?;
            places_inserted += 1;
        }
    }
    println!("places: inserted {places_inserted}");

    // ── manual_trip_merges ──────────────────────────────────────
    let mut merges_inserted = 0usize;
    let mut merges_skipped = 0usize;
    {
        let mut stmt = legacy.prepare(
            "SELECT primary_trip_id, absorbed_trip_id, created_ms FROM manual_trip_merges",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (legacy_primary, legacy_absorbed, created_ms) = row?;
            let Some(new_primary) = trip_map.get(&legacy_primary).cloned() else {
                merges_skipped += 1;
                continue;
            };
            let Some(new_absorbed) = trip_map.get(&legacy_absorbed).cloned() else {
                merges_skipped += 1;
                continue;
            };
            tx.execute(
                "INSERT OR IGNORE INTO manual_trip_merges (primary_trip_id, absorbed_trip_id, created_ms)
                 VALUES (?1, ?2, ?3)",
                params![new_primary, new_absorbed, created_ms],
            )?;
            merges_inserted += 1;
        }
    }
    println!("manual_trip_merges: inserted {merges_inserted} ({merges_skipped} skipped)");

    tx.commit()?;
    println!("\ndone.");
    Ok(())
}
