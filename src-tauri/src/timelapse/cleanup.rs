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

use rusqlite::{params, OptionalExtension};

use crate::app_settings::AppSettingsHandle;
use crate::db::timelapse_jobs::TimelapseJobRow;
use crate::db::{self, DbHandle};
use crate::error::AppError;
use crate::gps::GPS_PARSER_VERSION;
use crate::model::{derive_segment_id, derive_trip_id};
use crate::timelapse::worker;

/// Snapshot of every job still marked `running` in the DB. After a hard
/// process exit this is the set whose output files are partial and
/// whose rows need resetting before the next encode pass.
pub fn list_stale_jobs(db: &DbHandle) -> Result<Vec<TimelapseJobRow>, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::list_by_status(&conn, db::timelapse_jobs::STATUS_RUNNING)
}

/// Reset a single stale-running job: best-effort remove the partial
/// output file, then flip its DB row back to `pending`. Exposed so a
/// progress-reporting startup runner can loop over jobs and emit a
/// tick per recovered row.
pub fn reset_one_stale_job(
    db: &DbHandle,
    row: &TimelapseJobRow,
) -> Result<(), AppError> {
    if let Some(rel) = row.output_path.as_deref() {
        let archive_root = db.archive_root().to_path_buf();
        let abs = crate::paths::from_archive_relative(rel, &archive_root);
        if let Err(e) = fs::remove_file(&abs) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "[timelapse] cleanup: could not remove {}: {e} (continuing)",
                    abs.display()
                );
            }
        }
    }
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    db::timelapse_jobs::reset_to_pending(&conn, &row.trip_id, &row.tier, &row.channel)?;
    Ok(())
}

pub fn cleanup_stale_jobs(db: &DbHandle) -> Result<u64, AppError> {
    let running = list_stale_jobs(db)?;
    let count = running.len() as u64;
    for row in &running {
        reset_one_stale_job(db, row)?;
    }
    if count > 0 {
        eprintln!("[timelapse] cleanup: reset {count} stale running job(s) to pending");
    }
    wipe_scratch_tree(db);
    flag_missing_outputs(db)?;
    relink_present_outputs(db)?;
    backfill_output_sizes(db)?;
    Ok(count)
}

/// Recursively remove every entry under `<archive>/Timelapses/.tmp/`.
/// Run at startup so a previous session's hard exit (Ctrl-C, OS kill,
/// power loss) or a user-cancelled encode that couldn't sweep its own
/// scratch dir doesn't accumulate gigabytes of dead `__multi_window_*`
/// and `__multi_source.mp4` files.
///
/// Safe at startup because no encode can be running yet — the worker
/// pool hasn't started spawning ffmpeg children. Best-effort: failures
/// (permissions, file locks) are logged but never abort cleanup. The
/// `.tmp` directory itself is removed at the end if empty so a clean
/// archive doesn't leave a stub directory behind.
pub fn wipe_scratch_tree(db: &DbHandle) {
    let tmp_root = db.archive_root().join("Timelapses").join(".tmp");
    if !tmp_root.exists() {
        return;
    }
    let entries = match fs::read_dir(&tmp_root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[timelapse] cleanup: cannot read {}: {e} (skipping scratch wipe)",
                tmp_root.display()
            );
            return;
        }
    };
    let mut removed_dirs = 0u64;
    let mut removed_bytes = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        // Tally on the way in so the user-facing log line reports
        // something concrete rather than just a count.
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                removed_bytes += dir_size_bytes(&path);
            } else {
                removed_bytes += meta.len();
            }
        }
        let result = if entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed_dirs += 1,
            Err(e) => eprintln!(
                "[timelapse] cleanup: could not remove {}: {e}",
                path.display()
            ),
        }
    }
    // Best-effort empty-dir removal so we leave no stub behind.
    let _ = fs::remove_dir(&tmp_root);
    if removed_dirs > 0 {
        eprintln!(
            "[timelapse] cleanup: wiped {removed_dirs} leftover scratch entr{} ({} bytes reclaimed) from {}",
            if removed_dirs == 1 { "y" } else { "ies" },
            removed_bytes,
            tmp_root.display()
        );
    }
}

/// Sum the byte sizes of every regular file under `dir`. Best-effort;
/// unreadable entries are skipped. Used purely for the log line, so
/// missing data degrades gracefully into a smaller count.
fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let walk = match fs::read_dir(dir) {
        Ok(w) => w,
        Err(_) => return 0,
    };
    for entry in walk.flatten() {
        let path = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                total += dir_size_bytes(&path);
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Walk every `done` row, stat its output file, and flip any row
/// whose file no longer exists on disk to `failed` with a clear
/// error message. The next timelapse run picks them up via the
/// default "New & unfinished" scope and re-encodes.
///
/// This handles two real scenarios:
///   1. The user moved the archive to a different drive/mount. The
///      0013 migration rewrote stored paths to archive-relative,
///      which auto-resolves against the *current* mount; rows whose
///      files actually moved are fine, but rows whose files were
///      left behind on the old drive now read as "done" but point
///      at nothing.
///   2. A user manually deleted output files (e.g. `rm` from a
///      shell) without going through the app — same shape.
///
/// One stat per done row. Cheap — even libraries with thousands of
/// rows finish in well under a second on a local disk.
///
/// Called from both archive::open_archive (when the user picks an
/// archive via the switcher) AND startup::run (the background runner
/// invoked after auto-reopening the last archive at app start). The
/// two open paths bypass each other, so the housekeeping has to be
/// wired into both.
pub fn flag_missing_outputs(db: &DbHandle) -> Result<(), AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let candidates: Vec<(String, String, String, String)> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT trip_id, tier, channel, output_path
             FROM timelapse_jobs
             WHERE status = ?1
               AND output_path IS NOT NULL",
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
    if candidates.is_empty() {
        return Ok(());
    }
    // Safety guard for archives on removable/network mounts (this app's
    // common case). The DB lives inside the archive, so opening it proves
    // the root is mounted — but a transient or partial mount could leave
    // the Timelapses dir unreachable while done rows still reference it.
    // Without this, every `abs.exists()` would return false and we'd flip
    // the entire library to `failed`. If there are done outputs on record
    // but the Timelapses dir isn't there, treat the archive as not fully
    // reachable and skip — better to do nothing than to mass-fail.
    let timelapses_dir = archive_root.join("Timelapses");
    if !timelapses_dir.exists() {
        eprintln!(
            "[timelapse] cleanup: {} done row(s) on record but {} is unreachable — \
             skipping missing-output sweep (drive not fully mounted?)",
            candidates.len(),
            timelapses_dir.display()
        );
        return Ok(());
    }
    let mut flagged = 0u64;
    for (trip_id, tier, channel, output_path) in candidates {
        let abs = crate::paths::from_archive_relative(&output_path, &archive_root);
        if abs.exists() {
            continue;
        }
        let msg = format!(
            "output file missing on disk (expected at {})",
            abs.display()
        );
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        db::timelapse_jobs::mark_failed(&conn, &trip_id, &tier, &channel, &msg)?;
        flagged += 1;
    }
    if flagged > 0 {
        eprintln!(
            "[timelapse] cleanup: flagged {flagged} done row(s) as failed because their output file is missing"
        );
    }
    Ok(())
}

/// Inverse of `flag_missing_outputs`: walk every failed row whose
/// `output_path` is NULL and re-link it to the canonical on-disk file
/// (`<archive>/Timelapses/{trip_id}_{tier}_{channel}.mp4`) if that file
/// exists. Recovers from the `upsert_pending` bug where a "Rebuild all"
/// over an archive-only trip nulled the row's `output_path`, the rebuild
/// then failed with "no segments found for trip", and the perfectly good
/// on-disk MP4 became invisible to the UI even though it was still there.
///
/// Safe because:
///   - encode_trip_channel deletes partial output on every failure path,
///     so a file sitting at the canonical name is a real completed encode
///     rather than a half-written shred.
///   - We only touch rows already in `failed` state with `output_path`
///     NULL — rows with non-null `output_path` were already linked, and
///     genuinely-broken rows (output truly missing) stay failed.
///
/// Cheap: one stat per failed row (typically a few dozen at most). Other
/// encode metadata (speed_curve_json, encoder_used, ffmpeg_version,
/// padded_count) was wiped by the `upsert_pending` that started the
/// failed rebuild and can't be recovered — those columns stay NULL on
/// the re-linked row. Playback uses `output_path` + the file itself; the
/// metadata columns are only diagnostic.
///
/// Wired into both startup paths the same way `flag_missing_outputs` is:
/// `cleanup_stale_jobs` (for the archive-switcher open path) AND
/// `startup::run` (for the auto-reopen-last-archive path). Skipping
/// either leaves the recovery dependent on the user manually reopening
/// the archive, which is exactly the gap that delayed reclaim of the
/// original 27 rows.
pub fn relink_present_outputs(db: &DbHandle) -> Result<(), AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let candidates: Vec<(String, String, String)> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT trip_id, tier, channel
             FROM timelapse_jobs
             WHERE status = ?1
               AND output_path IS NULL",
        )?;
        let mapped = stmt.query_map(params![db::timelapse_jobs::STATUS_FAILED], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in mapped {
            out.push(r?);
        }
        out
    };
    if candidates.is_empty() {
        return Ok(());
    }
    let timelapses_dir = archive_root.join("Timelapses");
    let mut relinked = 0u64;
    for (trip_id, tier, channel) in candidates {
        let filename = format!("{trip_id}_{tier}_{channel}.mp4");
        let abs = timelapses_dir.join(&filename);
        let Ok(meta) = fs::metadata(&abs) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let stored = format!("Timelapses/{filename}");
        let size_bytes = meta.len() as i64;
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        conn.execute(
            "UPDATE timelapse_jobs SET
                status = ?4,
                output_path = ?5,
                error_message = NULL,
                output_size_bytes = ?6
             WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
            params![
                trip_id,
                tier,
                channel,
                db::timelapse_jobs::STATUS_DONE,
                stored,
                size_bytes,
            ],
        )?;
        relinked += 1;
    }
    if relinked > 0 {
        eprintln!(
            "[timelapse] cleanup: re-linked {relinked} failed row(s) to existing on-disk output file(s)"
        );
    }
    Ok(())
}

/// Best-effort recovery for orphan timelapse files left behind by the
/// archive-relative trip_id rewrite. When `rebuild_for_cross_os` ran,
/// it remapped every trip's UUID from "absolute-path-derived" to
/// "archive-relative-derived" — but it didn't rename the actual MP4
/// files on disk. After migration 0013 rewrote `output_path` values to
/// use the *new* trip_id, the DB row points at a filename that doesn't
/// exist while a file with the *old* trip_id sits next to it untouched.
///
/// This pass walks every row that `flag_missing_outputs` just flagged
/// as failed, reconstructs the candidate old absolute master_path for
/// every archive root the user has ever migrated through, recomputes
/// the old trip_id under each candidate, and renames any matching file
/// to the current trip_id naming. On success the DB row flips back to
/// done with the original speed curve / padded_count preserved
/// (`mark_failed` doesn't touch those columns).
///
/// Returns the number of files recovered. Safe to re-run: rows whose
/// file is already at the new location land as done immediately;
/// rows whose old file is also missing stay failed and the next
/// timelapse run will re-encode them.
pub fn recover_orphan_outputs(
    db: &DbHandle,
    app_settings: &AppSettingsHandle,
) -> Result<u64, AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let archive_root_str = archive_root.to_string_lossy().into_owned();
    let timelapses_dir = archive_root.join("Timelapses");

    // Candidate prefixes are every archive path the user has ever
    // opened (cross_os_migrated_archives + recent_archives), minus
    // the current root. We try them all because the user may have
    // moved the archive through more than one mount point and each
    // historical absolute path produces a distinct old trip_id.
    let candidates: Vec<String> = {
        let s = app_settings.read();
        let mut v: Vec<String> = s.cross_os_migrated_archives.clone();
        for ra in &s.recent_archives {
            if !v.contains(&ra.path) {
                v.push(ra.path.clone());
            }
        }
        v.retain(|p| *p != archive_root_str);
        v
    };
    if candidates.is_empty() {
        return Ok(0);
    }

    // Failed rows are the recovery candidates. We only look at rows
    // that flag_missing_outputs (or a worker run that hit a missing
    // sibling) has already marked failed — rows still in 'done' or
    // 'pending' have nothing to recover.
    let failed: Vec<(String, String, String)> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare(
            "SELECT trip_id, tier, channel FROM timelapse_jobs WHERE status = ?1",
        )?;
        let rows = stmt.query_map(params![db::timelapse_jobs::STATUS_FAILED], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out
    };
    if failed.is_empty() {
        return Ok(0);
    }

    let mut recovered = 0u64;
    for (trip_id, tier, channel) in &failed {
        // First segment of the trip — the master_path + start_time
        // pair the original UUID was derived from. Tombstones excluded
        // since they have an empty master_path.
        let first_seg: Option<(String, i64)> = {
            let conn = db
                .lock()
                .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
            conn.query_row(
                "SELECT master_path, start_time_ms FROM segments
                 WHERE trip_id = ?1 AND is_tombstone = 0
                 ORDER BY start_time_ms ASC LIMIT 1",
                params![trip_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("first-segment lookup: {e}")))?
        };
        let Some((rel_path, start_ms)) = first_seg else {
            continue;
        };
        let Some(start_time) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start_ms)
            .map(|dt| dt.naive_utc())
        else {
            continue;
        };

        let new_filename = format!("{trip_id}_{tier}_{channel}.mp4");
        let new_target = timelapses_dir.join(&new_filename);

        // Idempotency hatch: if the new-name file is already there
        // (e.g. a previous recovery pass renamed but the DB update
        // failed mid-flight), just refresh the row.
        if new_target.exists() {
            if let Err(e) = mark_recovered(db, trip_id, tier, channel, &new_filename, &new_target) {
                eprintln!("[timelapse] recovery: DB update failed for already-renamed file {}: {e}", new_target.display());
                continue;
            }
            recovered += 1;
            continue;
        }

        // Try each candidate old root. First hit wins.
        let mut found_for_row = false;
        for old_root in &candidates {
            let old_abs = build_old_absolute(old_root, &rel_path);
            let old_seg_id = derive_segment_id(&old_abs, start_time);
            let old_trip_id = derive_trip_id(old_seg_id).to_string();
            let old_filename = format!("{old_trip_id}_{tier}_{channel}.mp4");
            let candidate_path = timelapses_dir.join(&old_filename);
            if !candidate_path.exists() {
                continue;
            }
            match fs::rename(&candidate_path, &new_target) {
                Ok(()) => {
                    if let Err(e) =
                        mark_recovered(db, trip_id, tier, channel, &new_filename, &new_target)
                    {
                        eprintln!(
                            "[timelapse] recovery: file renamed but DB update failed for {} → {}: {e}",
                            candidate_path.display(),
                            new_target.display(),
                        );
                        // Leave the file at its new location — next
                        // pass's idempotency hatch picks it up.
                        found_for_row = true;
                        break;
                    }
                    recovered += 1;
                    found_for_row = true;
                    break;
                }
                Err(e) => {
                    eprintln!(
                        "[timelapse] recovery: rename {} → {} failed: {e}",
                        candidate_path.display(),
                        new_target.display(),
                    );
                    break;
                }
            }
        }
        let _ = found_for_row;
    }

    if recovered > 0 {
        eprintln!(
            "[timelapse] recovery: reclaimed {recovered} orphan output file(s) by renaming to current trip_id"
        );
    }
    Ok(recovered)
}

/// Reconstruct what an absolute master_path *would have been* when the
/// archive lived under `old_root`. The relative form stored today uses
/// '/' separators (per `to_archive_relative`), but the *original* UUID
/// was hashed from whatever string the OS produced when the file was
/// first scanned, which on Windows means backslashes.
///
/// To maximize the chance of reproducing the old hash, we use the
/// current host's `MAIN_SEPARATOR`. If the archive was originally
/// scanned on a different OS than the recovery is running on, the
/// derived UUID won't match and that file simply stays unrecovered —
/// the user falls back to re-encoding it. Mixed-OS hot-swaps are rare
/// and not worth a brute-force separator-permutation pass.
fn build_old_absolute(old_root: &str, rel_path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let trimmed = old_root.trim_end_matches(['/', '\\']);
    let rel_with_native = if sep == '/' {
        rel_path.to_string()
    } else {
        rel_path.replace('/', &sep.to_string())
    };
    format!("{trimmed}{sep}{rel_with_native}")
}

/// Summary returned by `prune_orphan_timelapse_files` so the caller can
/// surface what was reclaimed without re-walking the directory.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneSummary {
    /// Number of files that were moved to trash.
    pub trashed: u64,
    /// Bytes reclaimed (sum of file sizes BEFORE deletion).
    pub bytes_reclaimed: u64,
    /// Sample of the trashed filenames (capped, for UI display).
    pub sample: Vec<String>,
}

/// Count orphan timelapse files without deleting them. Same scan as
/// `prune_orphan_timelapse_files` but read-only — used by the
/// startup probe that decides whether to surface the Prune button
/// with a "pending action" badge. Cheap (one directory read + one
/// DB query); safe to call frequently.
pub fn count_orphan_timelapse_files(db: &DbHandle) -> Result<u64, AppError> {
    let timelapses_dir = db.archive_root().join("Timelapses");
    if !timelapses_dir.is_dir() {
        return Ok(0);
    }
    let referenced: std::collections::HashSet<String> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare("SELECT DISTINCT trip_id FROM timelapse_jobs")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut s = std::collections::HashSet::new();
        for r in rows {
            s.insert(r?);
        }
        s
    };
    let entries = match std::fs::read_dir(&timelapses_dir) {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };
    let mut count = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = filename.strip_suffix(".mp4") else {
            continue;
        };
        if stem.len() < 36 + 1 {
            continue;
        }
        let trip_id = &stem[..36];
        if !is_uuid_shape(trip_id) {
            continue;
        }
        if !referenced.contains(trip_id) {
            count += 1;
        }
    }
    Ok(count)
}

/// Move every on-disk file under `<archive_root>/Timelapses/` to trash
/// whose trip_id portion of the filename matches no row in
/// `timelapse_jobs`. These are the orphans left behind by an earlier
/// trip_id rewrite that didn't rename the files in lockstep — the DB
/// no longer references them and they would never be read by the app.
///
/// Files go to the OS trash via the existing `trash::delete` path so
/// the user can recover them if a misclassification is discovered
/// later. Not run automatically — invoked by the user via the
/// `prune_orphan_timelapse_files` Tauri command.
pub fn prune_orphan_timelapse_files(db: &DbHandle) -> Result<PruneSummary, AppError> {
    let archive_root = db.archive_root().to_path_buf();
    let timelapses_dir = archive_root.join("Timelapses");
    if !timelapses_dir.is_dir() {
        return Ok(PruneSummary {
            trashed: 0,
            bytes_reclaimed: 0,
            sample: Vec::new(),
        });
    }

    // Set of every trip_id the DB knows about, across any status. We
    // intentionally include pending/failed rows too — a row in any
    // state means the trip is still tracked and its file (if present)
    // should be left alone.
    let referenced: std::collections::HashSet<String> = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let mut stmt = conn.prepare("SELECT DISTINCT trip_id FROM timelapse_jobs")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut s = std::collections::HashSet::new();
        for r in rows {
            s.insert(r?);
        }
        s
    };

    let mut summary = PruneSummary {
        trashed: 0,
        bytes_reclaimed: 0,
        sample: Vec::new(),
    };
    const SAMPLE_CAP: usize = 8;

    let entries = match std::fs::read_dir(&timelapses_dir) {
        Ok(e) => e,
        Err(e) => {
            return Err(AppError::Internal(format!(
                "could not read {}: {e}",
                timelapses_dir.display()
            )))
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(f) => f,
            None => continue,
        };
        // Parse the trip_id portion: encoder names files
        // `{trip_id}_{tier}_{channel}.mp4` where trip_id is a 36-char
        // UUID. We only treat files matching that shape as candidates;
        // anything else (a `.tmp` scratch file, a user-renamed file,
        // etc.) is left alone.
        let Some(stem) = filename.strip_suffix(".mp4") else {
            continue;
        };
        if stem.len() < 36 + 1 {
            continue;
        }
        let trip_id = &stem[..36];
        if !is_uuid_shape(trip_id) {
            continue;
        }
        if referenced.contains(trip_id) {
            continue;
        }
        let size = std::fs::metadata(&path).ok().map(|m| m.len()).unwrap_or(0);
        match trash::delete(&path) {
            Ok(()) => {
                summary.trashed += 1;
                summary.bytes_reclaimed += size;
                if summary.sample.len() < SAMPLE_CAP {
                    summary.sample.push(filename.to_string());
                }
            }
            Err(e) => {
                eprintln!(
                    "[timelapse] prune: could not trash {}: {e} (continuing)",
                    path.display()
                );
            }
        }
    }
    if summary.trashed > 0 {
        eprintln!(
            "[timelapse] prune: moved {} orphan file(s) to trash ({} bytes reclaimed)",
            summary.trashed, summary.bytes_reclaimed
        );
    }
    Ok(summary)
}

/// Cheap UUID-shape check (8-4-4-4-12 hex). Strict enough to reject
/// arbitrary filenames the user might have dropped into Timelapses/,
/// loose enough to accept every UUIDv4/v5 the encoder produces.
fn is_uuid_shape(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    let dashes = [8, 13, 18, 23];
    for (i, b) in bytes.iter().enumerate() {
        if dashes.contains(&i) {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Flip a failed row back to done after its output file landed at the
/// canonical location. Preserves the original speed curve, padded count,
/// encoder identity, etc. — those columns were untouched by mark_failed
/// so they still reflect the encode that produced the file.
fn mark_recovered(
    db: &DbHandle,
    trip_id: &str,
    tier: &str,
    channel: &str,
    new_filename: &str,
    new_target: &std::path::Path,
) -> Result<(), AppError> {
    let stored = format!("Timelapses/{new_filename}");
    let size_bytes = fs::metadata(new_target).ok().map(|m| m.len() as i64);
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    conn.execute(
        "UPDATE timelapse_jobs SET
            status = ?4,
            output_path = ?5,
            error_message = NULL,
            output_size_bytes = COALESCE(?6, output_size_bytes)
         WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
        params![
            trip_id,
            tier,
            channel,
            db::timelapse_jobs::STATUS_DONE,
            stored,
            size_bytes,
        ],
    )?;
    Ok(())
}

/// One-shot pass to fill `output_size_bytes` on done rows that were
/// completed before migration 0009 (or whose output was missing at
/// completion time and is now present). Cheap — one stat per
/// completed job, dozens per typical library.
fn backfill_output_sizes(db: &DbHandle) -> Result<(), AppError> {
    let archive_root = db.archive_root().to_path_buf();
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
        // Stored archive-relative; rejoin so `fs::metadata` sees the
        // real file under the active mount.
        let abs = crate::paths::from_archive_relative(&output_path, &archive_root);
        let Ok(meta) = fs::metadata(&abs) else {
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

/// Opportunistic GPS archival for trips that were timelapsed before the
/// `trip_gps` feature shipped. Each launch we scan for at most
/// `max_trips` candidates and persist their stitched GPS so a future
/// "Delete originals" still leaves a working map + speed graph. Trips
/// whose originals are already gone (archive-only) are skipped — there
/// is nothing on disk to extract from.
///
/// Bounded so a cold-start on a heavy library doesn't tail-block the
/// app; it'll catch up across a handful of launches.
/// Sequential one-shot variant used by the test suite and by any
/// caller that doesn't need per-trip progress. The production startup
/// path goes through `startup::run`, which loops the two helpers
/// below and emits a `startup:task-progress` event after each trip.
#[allow(dead_code)]
pub fn backfill_trip_gps(db: &DbHandle, max_trips: usize) -> Result<usize, AppError> {
    let candidates = backfill_candidates(db, max_trips)?;
    if candidates.is_empty() {
        return Ok(0);
    }

    let mut written = 0usize;
    for trip_id in &candidates {
        if backfill_one_trip(db, trip_id)? {
            written += 1;
        }
    }

    if written > 0 {
        eprintln!("[timelapse] gps backfill: persisted GPS for {written} trip(s)");
    }
    Ok(written)
}

/// Trip ids that still need a `trip_gps` row written. The same selection
/// criteria as `backfill_trip_gps`: completed timelapse, at least one
/// non-tombstone segment on disk, no `trip_gps` row at the current
/// `parser_version`. Bounded by `max_trips` so a heavy library doesn't
/// pin startup on one launch.
pub fn backfill_candidates(
    db: &DbHandle,
    max_trips: usize,
) -> Result<Vec<String>, AppError> {
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.id FROM trips t
         WHERE EXISTS (
             SELECT 1 FROM timelapse_jobs j
             WHERE j.trip_id = t.id AND j.status = ?1
         )
         AND EXISTS (
             SELECT 1 FROM segments s
             WHERE s.trip_id = t.id AND s.is_tombstone = 0
         )
         AND NOT EXISTS (
             SELECT 1 FROM trip_gps g
             WHERE g.trip_id = t.id AND g.parser_version >= ?2
         )
         ORDER BY t.start_time_ms DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![
            db::timelapse_jobs::STATUS_DONE,
            GPS_PARSER_VERSION as i64,
            max_trips as i64
        ],
        |r| r.get::<_, String>(0),
    )?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Stitch and persist GPS for one trip. Returns `Ok(true)` when a
/// `trip_gps` row was written, `Ok(false)` when the trip yielded no
/// segments to stitch (caller can treat as a skip). Per-trip failures
/// during stitch or upsert are logged and folded into `Ok(false)` so a
/// single bad trip doesn't abort a sweep.
pub fn backfill_one_trip(db: &DbHandle, trip_id: &str) -> Result<bool, AppError> {
    let segments = match worker::trip_segment_info(db, trip_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[timelapse] gps backfill: trip_segment_info({trip_id}) failed: {e}");
            return Ok(false);
        }
    };
    if segments.is_empty() {
        return Ok(false);
    }
    let stitched = worker::stitch_trip_gps(&segments);
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    if let Err(e) = db::trip_gps::upsert(&conn, trip_id, &stitched, GPS_PARSER_VERSION) {
        eprintln!("[timelapse] gps backfill: upsert({trip_id}) failed: {e}");
        return Ok(false);
    }
    Ok(true)
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

    /// Regression for the orphan-file recovery path. Stages the exact
    /// shape that hit the user: an on-disk file named with an
    /// absolute-path-derived trip_id from an old mount point, a DB row
    /// using the new archive-relative-derived trip_id, and a settings
    /// file recording the old mount as a previously-migrated archive.
    /// The recovery pass should rename the file to the new trip_id
    /// naming and flip the row back to done.
    #[test]
    fn recovers_orphan_files_named_with_pre_rewrite_trip_id() {
        use crate::app_settings::AppSettingsHandle;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        let archive_root = temp_dir().join(format!("tripviewer-recover-{pid}-{n}"));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(archive_root.join("Timelapses")).unwrap();
        fs::create_dir_all(archive_root.join("Videos")).unwrap();

        // Simulate an "old" mount path. We don't need this directory
        // to exist on disk — only the string is used to recompute the
        // legacy UUID derivation.
        let old_root = temp_dir().join(format!("tripviewer-recover-old-{pid}-{n}"));
        let old_root_str = old_root.to_string_lossy().into_owned();

        // Segment metadata that both UUID schemes derive from.
        let rel_path = "Videos/2026_01_01_120000_00_F.MP4".to_string();
        let start_ms: i64 = 1_735_732_800_000; // 2025-01-01 12:00:00 UTC
        let start_time =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(start_ms)
                .unwrap()
                .naive_utc();

        // Compute the old UUID exactly the way the recovery pass will:
        // join the old root with the relative path using the host's
        // native separator. Mirrors `build_old_absolute`.
        let old_abs = format!(
            "{}{}{}",
            old_root_str.trim_end_matches(['/', '\\']),
            std::path::MAIN_SEPARATOR,
            if std::path::MAIN_SEPARATOR == '/' {
                rel_path.clone()
            } else {
                rel_path.replace('/', std::path::MAIN_SEPARATOR_STR)
            },
        );
        let old_seg_id = crate::model::derive_segment_id(&old_abs, start_time);
        let old_trip_id = crate::model::derive_trip_id(old_seg_id).to_string();

        // New UUID (what the DB has post-rewrite).
        let new_seg_id = crate::model::derive_segment_id(&rel_path, start_time);
        let new_trip_id = crate::model::derive_trip_id(new_seg_id).to_string();
        assert_ne!(
            old_trip_id, new_trip_id,
            "old and new trip_ids must differ for the test to be meaningful"
        );

        // Stage the orphan file at the OLD naming.
        let old_file = archive_root
            .join("Timelapses")
            .join(format!("{old_trip_id}_8x_F.mp4"));
        fs::write(&old_file, b"orphan-mp4-content").unwrap();

        // Stage the DB: trip + segment + a failed timelapse_jobs row.
        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind,
                    gps_supported, last_seen_ms)
                 VALUES (?1, ?2, ?3, 'wolfBox', 1, 0)",
                rusqlite::params![&new_trip_id, start_ms, start_ms + 60_000],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO segments (id, trip_id, start_time_ms, duration_s,
                    master_path, is_event, camera_kind, gps_supported, last_seen_ms)
                 VALUES (?1, ?2, ?3, 60.0, ?4, 0, 'wolfbox', 1, 0)",
                rusqlite::params![&new_seg_id.to_string(), &new_trip_id, start_ms, &rel_path],
            )
            .unwrap();
            db::timelapse_jobs::upsert_pending(&conn, &new_trip_id, "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn,
                &new_trip_id,
                "8x",
                "F",
                &format!("Timelapses/{new_trip_id}_8x_F.mp4"),
                "7.0",
                "hevc_nvenc",
                0,
                "[]",
                None,
            )
            .unwrap();
            db::timelapse_jobs::mark_failed(
                &conn,
                &new_trip_id,
                "8x",
                "F",
                "output file missing on disk (expected at <pre-recovery>)",
            )
            .unwrap();
        }

        // Stage settings: old root listed as a previously-migrated
        // archive (the exact place the recovery pass looks).
        let settings_dir = temp_dir().join(format!("tripviewer-recover-set-{pid}-{n}"));
        fs::create_dir_all(&settings_dir).unwrap();
        let settings_path = settings_dir.join("settings.json");
        fs::write(
            &settings_path,
            format!(
                r#"{{
                    "schema_version": 1,
                    "recent_archives": [],
                    "cross_os_migrated_archives": [{old_root}]
                }}"#,
                old_root = serde_json::to_string(&old_root_str).unwrap(),
            ),
        )
        .unwrap();
        let settings = AppSettingsHandle::load(&settings_dir);

        let recovered = recover_orphan_outputs(&db, &settings).unwrap();
        assert_eq!(recovered, 1);

        // File now lives at the new naming.
        let new_file = archive_root
            .join("Timelapses")
            .join(format!("{new_trip_id}_8x_F.mp4"));
        assert!(new_file.exists(), "renamed file must exist at new path");
        assert!(!old_file.exists(), "original old-named file must be gone");

        // Row flipped back to done with the canonical relative output_path.
        let conn = db.lock().unwrap();
        let row = db::timelapse_jobs::get(&conn, &new_trip_id, "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, db::timelapse_jobs::STATUS_DONE);
        assert_eq!(
            row.output_path.as_deref(),
            Some(format!("Timelapses/{new_trip_id}_8x_F.mp4").as_str())
        );
        assert!(row.error_message.is_none());
        // mark_failed never touched the encode metadata, so the
        // speed curve / encoder identity survives the round trip.
        assert_eq!(row.encoder_used.as_deref(), Some("hevc_nvenc"));

        drop(conn);
        let _ = fs::remove_dir_all(&archive_root);
        let _ = fs::remove_dir_all(&settings_dir);
    }

    /// Regression for the "lying done" state: a done row whose
    /// output file isn't on disk anymore (e.g. user moved the archive
    /// drive and only some files came along). The startup sweep must
    /// flip those rows to failed so the next New & unfinished run
    /// picks them up.
    #[test]
    fn flags_done_rows_whose_files_are_missing() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let archive_root = temp_dir().join(format!(
            "tripviewer-cleanup-miss-{}-{}",
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(archive_root.join("Timelapses")).unwrap();
        // Present file: this done row must stay 'done'.
        let present = archive_root.join("Timelapses").join("present_8x_F.mp4");
        fs::write(&present, b"").unwrap();

        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, "present", "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn,
                "present",
                "8x",
                "F",
                "Timelapses/present_8x_F.mp4",
                "7.0",
                "hevc_nvenc",
                0,
                "[]",
                None,
            )
            .unwrap();

            db::timelapse_jobs::upsert_pending(&conn, "missing", "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn,
                "missing",
                "8x",
                "F",
                "Timelapses/missing_8x_F.mp4",
                "7.0",
                "hevc_nvenc",
                0,
                "[]",
                None,
            )
            .unwrap();
        }

        cleanup_stale_jobs(&db).unwrap();

        let conn = db.lock().unwrap();
        let present_row = db::timelapse_jobs::get(&conn, "present", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(present_row.status, db::timelapse_jobs::STATUS_DONE);
        let missing_row = db::timelapse_jobs::get(&conn, "missing", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(missing_row.status, db::timelapse_jobs::STATUS_FAILED);
        assert!(missing_row
            .error_message
            .as_deref()
            .unwrap_or("")
            .contains("missing on disk"));

        drop(conn);
        let _ = fs::remove_dir_all(&archive_root);
    }

    /// Unplugged/partial-mount guard: if done rows reference outputs but
    /// the Timelapses dir isn't present (drive not fully mounted), the
    /// sweep must NOT flip the whole library to failed — it does nothing.
    #[test]
    fn does_not_flag_when_timelapses_dir_is_unreachable() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let archive_root = temp_dir().join(format!(
            "tripviewer-cleanup-unmount-{}-{}",
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&archive_root);
        // Archive root exists (DB opened) but NO Timelapses dir — mimics
        // a partial mount where outputs are unreachable.
        fs::create_dir_all(&archive_root).unwrap();
        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, "t", "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn, "t", "8x", "F", "Timelapses/t_8x_F.mp4", "7.0", "hevc_nvenc", 0, "[]", None,
            )
            .unwrap();
        }

        flag_missing_outputs(&db).unwrap();

        let conn = db.lock().unwrap();
        let row = db::timelapse_jobs::get(&conn, "t", "8x", "F").unwrap().unwrap();
        assert_eq!(
            row.status,
            db::timelapse_jobs::STATUS_DONE,
            "must not flip done→failed when the Timelapses dir is unreachable"
        );
        drop(conn);
        let _ = fs::remove_dir_all(&archive_root);
    }

    /// Regression for the upsert_pending bug: a failed row with NULL
    /// output_path whose expected on-disk file is actually present must
    /// be flipped back to done with the canonical relative output_path.
    /// Failed rows whose file is truly missing stay failed; done rows
    /// are never touched.
    #[test]
    fn relinks_failed_rows_whose_files_are_actually_present() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let archive_root = temp_dir().join(format!("tripviewer-relink-{pid}-{n}"));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(archive_root.join("Timelapses")).unwrap();

        // File-on-disk for a row that was failed by the bug.
        let present = archive_root
            .join("Timelapses")
            .join("recover_8x_F.mp4");
        fs::write(&present, b"existing-encoded-content").unwrap();
        let present_size = fs::metadata(&present).unwrap().len();

        // Untouched done row whose file is also present — must not be
        // disturbed by the relink pass.
        let untouched = archive_root
            .join("Timelapses")
            .join("untouched_8x_F.mp4");
        fs::write(&untouched, b"already-linked").unwrap();

        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        {
            let conn = db.lock().unwrap();

            // Row mirroring the upsert_pending bug aftermath: status
            // failed with the exact error message the worker produces,
            // output_path NULL.
            db::timelapse_jobs::upsert_pending(&conn, "recover", "8x", "F").unwrap();
            db::timelapse_jobs::mark_failed(
                &conn,
                "recover",
                "8x",
                "F",
                "no segments found for trip",
            )
            .unwrap();

            // Failed row whose file is truly missing — must stay failed.
            db::timelapse_jobs::upsert_pending(&conn, "truly_gone", "8x", "F").unwrap();
            db::timelapse_jobs::mark_failed(
                &conn,
                "truly_gone",
                "8x",
                "F",
                "no segments found for trip",
            )
            .unwrap();

            // Untouched done row.
            db::timelapse_jobs::upsert_pending(&conn, "untouched", "8x", "F").unwrap();
            db::timelapse_jobs::mark_done(
                &conn,
                "untouched",
                "8x",
                "F",
                "Timelapses/untouched_8x_F.mp4",
                "7.0",
                "hevc_nvenc",
                0,
                "[]",
                Some(14),
            )
            .unwrap();
        }

        relink_present_outputs(&db).unwrap();

        let conn = db.lock().unwrap();
        let recovered = db::timelapse_jobs::get(&conn, "recover", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(recovered.status, db::timelapse_jobs::STATUS_DONE);
        assert_eq!(
            recovered.output_path.as_deref(),
            Some("Timelapses/recover_8x_F.mp4")
        );
        assert!(recovered.error_message.is_none());
        assert_eq!(recovered.output_size_bytes, Some(present_size as i64));

        let still_gone = db::timelapse_jobs::get(&conn, "truly_gone", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(still_gone.status, db::timelapse_jobs::STATUS_FAILED);
        assert!(still_gone.output_path.is_none());

        let untouched_row = db::timelapse_jobs::get(&conn, "untouched", "8x", "F")
            .unwrap()
            .unwrap();
        assert_eq!(untouched_row.status, db::timelapse_jobs::STATUS_DONE);
        assert_eq!(
            untouched_row.output_path.as_deref(),
            Some("Timelapses/untouched_8x_F.mp4")
        );

        drop(conn);
        let _ = fs::remove_dir_all(&archive_root);
    }

    /// Verifies the startup scratch-wipe: every entry under
    /// <archive>/Timelapses/.tmp/ — whether a leftover per-job scratch
    /// directory or a stray file — gets removed regardless of size or
    /// nesting depth. Files outside .tmp/ are untouched. The function
    /// is a no-op when .tmp/ doesn't exist.
    #[test]
    fn wipe_scratch_tree_removes_leftover_scratch_dirs() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let archive_root = temp_dir().join(format!("tripviewer-wipe-{pid}-{n}"));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(archive_root.join("Timelapses").join(".tmp")).unwrap();

        // Stage a couple of leftover scratch dirs with content, plus a
        // stray top-level file. All must be gone after the sweep.
        let job1_dir = archive_root
            .join("Timelapses")
            .join(".tmp")
            .join("trip-1_8x_F");
        fs::create_dir_all(&job1_dir).unwrap();
        fs::write(job1_dir.join("__multi_source.mp4"), b"abc").unwrap();
        fs::write(job1_dir.join("__multi_window_0.mp4"), b"def").unwrap();
        let job2_dir = archive_root
            .join("Timelapses")
            .join(".tmp")
            .join("trip-2_16x_R");
        fs::create_dir_all(&job2_dir).unwrap();
        fs::write(job2_dir.join("__multi_source.mp4"), b"ghi").unwrap();
        let stray_file = archive_root
            .join("Timelapses")
            .join(".tmp")
            .join("stray.mp4");
        fs::write(&stray_file, b"jkl").unwrap();
        // A real (non-scratch) timelapse file at the Timelapses root —
        // must NOT be touched.
        let real = archive_root.join("Timelapses").join("preserved.mp4");
        fs::write(&real, b"keep me").unwrap();

        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        wipe_scratch_tree(&db);

        assert!(!job1_dir.exists(), "leftover job dir 1 must be removed");
        assert!(!job2_dir.exists(), "leftover job dir 2 must be removed");
        assert!(!stray_file.exists(), "stray file in .tmp must be removed");
        // .tmp itself goes too — empty directory cleanup.
        assert!(
            !archive_root.join("Timelapses").join(".tmp").exists(),
            ".tmp/ should be removed when empty"
        );
        assert!(real.exists(), "files outside .tmp/ must be preserved");

        // Idempotent — running again on a clean tree is a no-op.
        wipe_scratch_tree(&db);

        let _ = fs::remove_dir_all(&archive_root);
    }

    /// Verifies the orphan-prune logic: files whose trip_id matches a
    /// row in timelapse_jobs are kept, files whose trip_id matches
    /// nothing in the DB get moved to trash, non-UUID-shaped filenames
    /// are ignored. Uses `trash::delete` so the test is somewhat
    /// environment-dependent (XDG trash on Linux, recycle bin on
    /// Windows); we just assert the file is gone from its original
    /// location, not where it ended up.
    #[test]
    fn prune_orphan_timelapse_files_removes_unreferenced_files_only() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let archive_root = temp_dir().join(format!("tripviewer-prune-{pid}-{n}"));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(archive_root.join("Timelapses")).unwrap();

        // referenced_trip's file is named with a trip_id present in the
        // DB → must survive prune.
        let referenced = "11111111-2222-3333-4444-555555555555";
        let referenced_file = archive_root
            .join("Timelapses")
            .join(format!("{referenced}_8x_F.mp4"));
        fs::write(&referenced_file, b"keep").unwrap();

        // orphan_trip's file is named with a trip_id NOT in the DB.
        let orphan = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let orphan_file = archive_root
            .join("Timelapses")
            .join(format!("{orphan}_8x_F.mp4"));
        fs::write(&orphan_file, b"orphan").unwrap();

        // Non-UUID-shaped filename — must be left alone regardless of
        // whether its prefix happens to match a DB row.
        let weird_file = archive_root
            .join("Timelapses")
            .join("notes-from-the-user.mp4");
        fs::write(&weird_file, b"user file").unwrap();

        // Subdirectory — must be skipped (we only consider files).
        let subdir = archive_root.join("Timelapses").join("debug");
        fs::create_dir_all(&subdir).unwrap();
        let inside = subdir.join(format!("{orphan}_8x_F.mp4"));
        fs::write(&inside, b"inside subdir").unwrap();

        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        {
            let conn = db.lock().unwrap();
            db::timelapse_jobs::upsert_pending(&conn, referenced, "8x", "F").unwrap();
        }

        let summary = prune_orphan_timelapse_files(&db).unwrap();
        assert_eq!(summary.trashed, 1, "exactly one orphan should be trashed");
        assert!(referenced_file.exists(), "DB-referenced file must survive");
        assert!(!orphan_file.exists(), "orphan file must be gone from disk");
        assert!(
            weird_file.exists(),
            "non-UUID-shaped files must not be touched"
        );
        assert!(inside.exists(), "subdirectory entries must not be touched");
        assert!(summary.bytes_reclaimed > 0);
        assert_eq!(summary.sample, vec![format!("{orphan}_8x_F.mp4")]);

        let _ = fs::remove_dir_all(&archive_root);
    }

    #[test]
    fn prune_orphan_timelapse_files_handles_missing_directory() {
        // Archive with no Timelapses/ folder yet — function should
        // return zeroes rather than erroring on the missing dir.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let archive_root = temp_dir().join(format!("tripviewer-prune-empty-{pid}-{n}"));
        let _ = fs::remove_dir_all(&archive_root);
        fs::create_dir_all(&archive_root).unwrap();

        let db = crate::db::open_in_memory_with_root(&archive_root).unwrap();
        let summary = prune_orphan_timelapse_files(&db).unwrap();
        assert_eq!(summary.trashed, 0);
        assert_eq!(summary.bytes_reclaimed, 0);
        assert!(summary.sample.is_empty());

        let _ = fs::remove_dir_all(&archive_root);
    }
}
