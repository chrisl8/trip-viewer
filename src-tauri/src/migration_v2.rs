//! Per-archive DB migration: relocate the legacy single-archive DB at
//! `app_data_dir/tripviewer.db` into `<archive_root>/.tripviewer/tripviewer.db`
//! so the user's metadata travels with their video drive.
//!
//! Triggered on launch when:
//! - the legacy file at `app_data_dir/tripviewer.db` exists, AND
//! - `AppSettings.last_archive` is `None`, AND
//! - we can derive an archive root from the legacy DB's segments.
//!
//! Approach: rename the file in place after a WAL checkpoint. No path
//! rewriting yet — the absolute paths inside the DB stay valid because
//! the user is migrating on the same machine. Cross-OS portability is
//! a separate change that wires the `paths::to/from_archive_relative`
//! helpers into every read/write site.
//!
//! UX-wise this is **silent** when discovery succeeds: the legacy DB
//! has segments whose master_path lives under a `Videos/` folder, so
//! we know the user's archive root without asking. PR 3 will add an
//! explicit "Open archive…" picker so users in the edge case (no
//! segments, or non-standard layout) can pick a folder themselves.
//! Showing a dialog here is not an option — Tauri's blocking dialog
//! API requires the event loop, which the `setup()` callback runs
//! before.
//!
//! The migration is idempotent: if it can't run (no legacy DB, or
//! discovery fails), the legacy file stays where it is and we re-check
//! every launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::app_settings::AppSettingsHandle;
use crate::db::DbHandle;
use crate::error::AppError;
use crate::model::{derive_segment_id, derive_trip_id};
use crate::paths;
use crate::timelapse::worker::{discover_library_root, DiscoveredRoot};

/// Outcome of a migration attempt. The string is logged to stderr for
/// post-launch debugging; the frontend doesn't see this directly.
pub enum MigrationOutcome {
    NotNeeded,
    Migrated { archive_root: PathBuf },
    Skipped { reason: String },
}

/// Run the per-archive migration if the launch state warrants it.
pub fn run_if_needed(
    app_data_dir: &Path,
    settings: &AppSettingsHandle,
) -> Result<MigrationOutcome, AppError> {
    let legacy_db_path = app_data_dir.join("tripviewer.db");
    if !legacy_db_path.exists() {
        return Ok(MigrationOutcome::NotNeeded);
    }
    if settings.read().last_archive.is_some() {
        return Ok(MigrationOutcome::NotNeeded);
    }

    // Pull any unmigrated per-machine settings out of the legacy DB
    // before we move it. Idempotent (gated by schema_version inside
    // app_settings) — for users who already went through the JSON
    // settings migration this is a no-op.
    {
        let conn = Connection::open(&legacy_db_path)?;
        if let Err(e) = crate::app_settings::migrate_from_sqlite(settings, &conn) {
            eprintln!("[migration_v2] settings extraction failed: {e}");
        }
    }

    // Derive the archive root from the legacy DB's segments. Only the
    // `Library` variant (parent-of-Videos) is treated as confident
    // enough to auto-migrate: that's the structured layout the import
    // pipeline produces. `SegmentParent` (no Videos/ ancestor) means
    // the user scanned in place from an arbitrary folder; auto-moving
    // their DB into a hidden subfolder there would be surprising, so
    // we leave it alone and let PR 3's archive picker handle that
    // case explicitly.
    let archive_root: PathBuf = {
        let conn = Connection::open(&legacy_db_path)?;
        match discover_library_root(&conn) {
            Ok(DiscoveredRoot::Library(p)) => p,
            Ok(DiscoveredRoot::SegmentParent(p)) => {
                return Ok(MigrationOutcome::Skipped {
                    reason: format!(
                        "segments live under {} but no Videos/ ancestor — \
                         auto-migration only handles structured archives. \
                         Use the upcoming Open Archive UI.",
                        p.display()
                    ),
                });
            }
            Err(e) => {
                return Ok(MigrationOutcome::Skipped {
                    reason: format!("could not derive archive root from segments: {e}"),
                });
            }
        }
    };

    // Pre-flight: refuse if the chosen folder already has a per-archive
    // DB. Better safe than overwriting.
    let new_db_dir = archive_root.join(".tripviewer");
    let new_db_path = new_db_dir.join("tripviewer.db");
    if new_db_path.exists() {
        return Ok(MigrationOutcome::Skipped {
            reason: format!(
                "target archive already has a Trip Viewer DB at {}: \
                 refusing to overwrite",
                new_db_path.display()
            ),
        });
    }

    // Checkpoint the WAL into the main file so the rename is safe to
    // do on the .db alone. Without this, an outstanding WAL would
    // either be orphaned at the legacy location (lost) or copied to
    // the new location (race-prone).
    {
        let conn = Connection::open(&legacy_db_path)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    }

    std::fs::create_dir_all(&new_db_dir)?;

    // Copy first, verify, then delete the source. `fs::rename` only
    // works within a single filesystem; the typical migration moves
    // from the OS drive (~/.local/share) onto the user's NTFS/ext4
    // archive volume, which is a different mount, and would fail with
    // EXDEV. Copy is also a safer pattern for irreplaceable data —
    // a torn copy can be retried, but a half-moved rename can't.
    let src_segments: i64 = {
        let conn = Connection::open(&legacy_db_path)?;
        conn.query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0))?
    };

    std::fs::copy(&legacy_db_path, &new_db_path)?;

    // Sanity-check the destination before unlinking the source. If
    // anything's off (truncated copy, schema corruption, segment count
    // mismatch), roll back the destination and leave the legacy file
    // for retry next launch.
    let dest_segments: i64 = match Connection::open(&new_db_path) {
        Ok(conn) => match conn.query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0)) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&new_db_path);
                return Err(AppError::Internal(format!(
                    "destination DB unreadable after copy, rolled back: {e}"
                )));
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&new_db_path);
            return Err(AppError::Internal(format!(
                "destination DB unopenable after copy, rolled back: {e}"
            )));
        }
    };
    if dest_segments != src_segments {
        let _ = std::fs::remove_file(&new_db_path);
        return Err(AppError::Internal(format!(
            "segment count mismatch after copy: src={src_segments}, dest={dest_segments}"
        )));
    }

    // Self-check passed. Now safe to delete the legacy file.
    std::fs::remove_file(&legacy_db_path)?;

    // Sweep stale WAL/SHM files left at the legacy location. After
    // wal_checkpoint(TRUNCATE) and the source delete these have no
    // purpose; SQLite will recreate fresh ones at the new location.
    for ext in ["-wal", "-shm"] {
        let stale = app_data_dir.join(format!("tripviewer.db{ext}"));
        if stale.exists() {
            let _ = std::fs::remove_file(stale);
        }
    }

    settings.update(|s| {
        s.last_archive = Some(archive_root.to_string_lossy().into_owned());
    })?;

    Ok(MigrationOutcome::Migrated { archive_root })
}

/// Rewrite `segments.master_path` to archive-relative form *and*
/// recompute every segment / trip UUID off the new path so the IDs
/// are stable when the same archive is opened on a different OS.
///
/// Cascades through every FK column so tag and timelapse-job links
/// survive: `tags.segment_id`, `tags.trip_id`, `scan_runs.segment_id`,
/// `segments.trip_id`, `timelapse_jobs.trip_id`,
/// `manual_trip_merges.{primary,absorbed}_trip_id`.
///
/// Idempotent at the call site: gated by
/// `AppSettings.cross_os_migrated_archives` so it runs at most once
/// per archive root. Within the function we still skip rows that
/// don't need rewriting (path already relative, or path lives outside
/// the archive root).
pub fn rebuild_for_cross_os(
    db: &DbHandle,
    settings: &AppSettingsHandle,
) -> Result<RebuildOutcome, AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let archive_str = archive_root.to_string_lossy().into_owned();

    if settings
        .read()
        .cross_os_migrated_archives
        .iter()
        .any(|p| p == &archive_str)
    {
        return Ok(RebuildOutcome::AlreadyDone);
    }

    let segments_remapped = rebuild_in_transaction(db, &archive_str)?;

    settings.update(|s| {
        if !s.cross_os_migrated_archives.iter().any(|p| p == &archive_str) {
            s.cross_os_migrated_archives.push(archive_str.clone());
        }
    })?;

    Ok(RebuildOutcome::Migrated { segments_remapped })
}

pub enum RebuildOutcome {
    AlreadyDone,
    Migrated { segments_remapped: usize },
}

fn rebuild_in_transaction(db: &DbHandle, archive_str: &str) -> Result<usize, AppError> {
    // Snapshot rows we may need to remap. Done with a short-lived lock
    // so we can release before re-locking for the transaction; SQLite
    // doesn't support nested transactions and `db.lock()` returns the
    // single shared connection.
    struct SegRow {
        id: String,
        master_path: String,
        start_ms: i64,
    }
    struct TripRow {
        id: String,
        first_seg_id: Option<String>,
    }

    let (segs, trips) = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let segs: Vec<SegRow> = {
            let mut stmt = conn.prepare(
                "SELECT id, master_path, start_time_ms FROM segments
                 WHERE is_tombstone = 0 AND master_path != ''",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(SegRow {
                    id: r.get(0)?,
                    master_path: r.get(1)?,
                    start_ms: r.get(2)?,
                })
            })?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        let trips: Vec<TripRow> = {
            let mut stmt = conn.prepare(
                "SELECT t.id,
                        (SELECT s.id FROM segments s
                         WHERE s.trip_id = t.id
                         ORDER BY s.start_time_ms LIMIT 1) AS first_seg_id
                 FROM trips t",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(TripRow {
                    id: r.get(0)?,
                    first_seg_id: r.get(1)?,
                })
            })?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        (segs, trips)
    };

    let mut seg_id_remap: HashMap<String, String> = HashMap::new();
    let mut seg_path_remap: HashMap<String, String> = HashMap::new();

    for s in &segs {
        let Some(rel) = paths::relativize_str(&s.master_path, archive_str) else {
            // Path doesn't live under archive_root — leave the row
            // alone. Its UUID stays absolute-derived, which is OS-
            // specific but at least internally consistent. PR 3's
            // archive switcher gives users a way to relocate these.
            continue;
        };
        let already_relative = !Path::new(&s.master_path).is_absolute();
        if already_relative && rel == s.master_path {
            // Nothing to do — already in storage form.
            continue;
        }
        let start_time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(s.start_ms)
            .ok_or_else(|| AppError::Internal(format!("invalid start_time_ms {}", s.start_ms)))?
            .naive_utc();
        let new_id = derive_segment_id(&rel, start_time).to_string();
        if new_id == s.id {
            // UUID happens to land on the same value (path unchanged).
            // Skip cascade work for this row.
            continue;
        }
        seg_id_remap.insert(s.id.clone(), new_id);
        seg_path_remap.insert(s.id.clone(), rel);
    }

    let mut trip_id_remap: HashMap<String, String> = HashMap::new();
    for t in &trips {
        let Some(first_old) = t.first_seg_id.as_ref() else {
            continue;
        };
        let Some(first_new) = seg_id_remap.get(first_old) else {
            continue;
        };
        let new_first_uuid = Uuid::parse_str(first_new).map_err(|e| {
            AppError::Internal(format!("non-UUID in segment id mapping {first_new}: {e}"))
        })?;
        let new_trip_id = derive_trip_id(new_first_uuid).to_string();
        if new_trip_id == t.id {
            continue;
        }
        trip_id_remap.insert(t.id.clone(), new_trip_id);
    }

    if seg_id_remap.is_empty() && trip_id_remap.is_empty() {
        return Ok(0);
    }

    let mut conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let tx = conn.transaction()?;

    // Cascade FK-style references first (while old IDs still resolve
    // in segments/trips). No FK CONSTRAINTs in our schema, so the
    // ordering is for code-level consistency rather than DB safety.
    for (old_id, new_id) in &seg_id_remap {
        tx.execute(
            "UPDATE tags SET segment_id = ?1 WHERE segment_id = ?2",
            params![new_id, old_id],
        )?;
        tx.execute(
            "UPDATE scan_runs SET segment_id = ?1 WHERE segment_id = ?2",
            params![new_id, old_id],
        )?;
    }
    for (old_id, new_id) in &trip_id_remap {
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

    // Now update the canonical rows. SQLite allows updating a PK
    // directly as long as no UNIQUE conflict exists; UUID collisions
    // between old and new are astronomically unlikely.
    for (old_id, new_id) in &seg_id_remap {
        let new_path = seg_path_remap
            .get(old_id)
            .expect("seg_id_remap and seg_path_remap built together");
        tx.execute(
            "UPDATE segments SET id = ?1, master_path = ?2 WHERE id = ?3",
            params![new_id, new_path, old_id],
        )?;
    }
    for (old_id, new_id) in &trip_id_remap {
        tx.execute(
            "UPDATE trips SET id = ?1 WHERE id = ?2",
            params![new_id, old_id],
        )?;
    }

    tx.commit()?;
    Ok(seg_id_remap.len())
}

/// One-shot cleanup of orphan files in `app_data_dir` that previous
/// versions of Trip Viewer left behind.
pub fn cleanup_orphan_files(app_data_dir: &Path) {
    // recovery-config.json — referenced nowhere in current Rust or
    // TS code (verified via grep). Delete on sight.
    let orphan = app_data_dir.join("recovery-config.json");
    if orphan.exists() {
        match std::fs::remove_file(&orphan) {
            Ok(()) => eprintln!("[migration_v2] removed orphan {}", orphan.display()),
            Err(e) => eprintln!("[migration_v2] could not remove {}: {e}", orphan.display()),
        }
    }
}
