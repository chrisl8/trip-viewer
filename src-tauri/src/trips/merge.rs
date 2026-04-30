//! Manual trip-merge backend. Handles the user's "join these trips into
//! one" action, including timelapse output handling.
//!
//! Two pieces of public surface:
//! - `assess_timelapse_merge` — read-only, reports per-(tier, channel)
//!   what's possible with the existing outputs (concatenate them
//!   losslessly, partial coverage, none). The frontend uses this to
//!   choose between strategies.
//! - `merge_trips` — performs the merge: rewrites segments + tags +
//!   timelapse_jobs to point at the primary trip, optionally concats
//!   matching timelapse outputs, records a directive in
//!   `manual_trip_merges` so the merge survives a folder rescan, and
//!   rebuilds the primary's `trips` row to span the union.
//!
//! Cross-camera merges aren't blocked here. ffmpeg's concat will refuse
//! mismatched codecs / resolutions / pix_fmt at encode time. The
//! assessment surfaces this pre-merge by returning the distinct set of
//! `camera_kind` values across primary + absorbed in `camera_kinds` —
//! the dialog warns when more than one is present so the user knows
//! concat will fail and they should pick `discardAll` (or split the
//! marked set).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use crate::timelapse::ffmpeg::ffmpeg_command;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::DbHandle;
use crate::error::AppError;
use crate::timelapse::speed_curve::CurveSegment;

const SETTING_FFMPEG_PATH: &str = "ffmpeg_path";
const SETTING_LIBRARY_ROOT: &str = "library_root";

// ── Public types (also serialized for IPC) ──────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TupleStatus {
    /// Every source trip (primary + all absorbed) has a `done` output
    /// for this (tier, channel). Concat is feasible.
    Concatenable,
    /// At least one — but not every — source has a done output. Concat
    /// would produce a coherent file only for the parts of the merged
    /// trip that have inputs; we treat this as not-concatable and
    /// require a rebuild for the merged trip to have full coverage.
    PartialOutputs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TupleAssessment {
    pub tier: String,
    pub channel: String,
    pub status: TupleStatus,
    pub primary_has: bool,
    /// Absorbed trip IDs that have a done output for this tuple.
    pub absorbed_with_output: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelapseMergeAssessment {
    /// True if any source trip (primary or absorbed) has at least one
    /// timelapse_jobs row. When false the frontend can skip the dialog
    /// and merge silently.
    pub has_any_timelapses: bool,
    pub tuples: Vec<TupleAssessment>,
    /// Distinct `camera_kind` values across primary + absorbed, sorted.
    /// More than one entry means the merge crosses camera brands and
    /// any concat will fail at ffmpeg time on resolution / pix_fmt
    /// mismatches — the frontend surfaces a warning before the user
    /// commits.
    pub camera_kinds: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TimelapseMergeStrategy {
    /// For each (tier, channel) tuple where every source has a done
    /// output, concat them losslessly into a single output for the
    /// primary. Tuples where only some sources have outputs are
    /// dropped — user can click Rebuild on the merged trip.
    ConcatWherePossible,
    /// Delete every timelapse_jobs row for primary + absorbed. Merged
    /// trip starts with no encoded outputs; user rebuilds.
    DiscardAll,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    pub primary_trip_id: String,
    pub absorbed_trip_ids: Vec<String>,
    /// (tier, channel) tuples that were successfully concatenated.
    pub concatenated: Vec<(String, String)>,
    /// Total `timelapse_jobs` rows removed (failed concats, partial
    /// tuples, or every row when strategy is DiscardAll).
    pub timelapse_jobs_removed: usize,
}

// ── Assessment ──────────────────────────────────────────────────────

#[derive(Debug)]
struct JobRow {
    trip_id: String,
    tier: String,
    channel: String,
    status: String,
    output_path: Option<String>,
    speed_curve_json: Option<String>,
    padded_count: i64,
    encoder_used: Option<String>,
    ffmpeg_version: Option<String>,
}

fn load_job_rows(
    conn: &Connection,
    trip_ids: &[String],
) -> Result<Vec<JobRow>, AppError> {
    if trip_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", trip_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT trip_id, tier, channel, status, output_path, speed_curve_json,
                padded_count, encoder_used, ffmpeg_version
         FROM timelapse_jobs WHERE trip_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(trip_ids.iter()), |r| {
        Ok(JobRow {
            trip_id: r.get(0)?,
            tier: r.get(1)?,
            channel: r.get(2)?,
            status: r.get(3)?,
            output_path: r.get(4)?,
            speed_curve_json: r.get(5)?,
            padded_count: r.get(6)?,
            encoder_used: r.get(7)?,
            ffmpeg_version: r.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn assess_timelapse_merge(
    db: &DbHandle,
    primary: Uuid,
    absorbed: &[Uuid],
) -> Result<TimelapseMergeAssessment, AppError> {
    let mut all_ids: Vec<String> = absorbed.iter().map(|u| u.to_string()).collect();
    all_ids.push(primary.to_string());

    let (rows, camera_kinds) = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let rows = load_job_rows(&conn, &all_ids)?;
        let kinds = load_camera_kinds(&conn, &all_ids)?;
        (rows, kinds)
    };

    if rows.is_empty() {
        return Ok(TimelapseMergeAssessment {
            has_any_timelapses: false,
            tuples: Vec::new(),
            camera_kinds,
        });
    }

    // Group "done" rows by (tier, channel). Other statuses don't
    // contribute to concat feasibility.
    let mut tuples: HashMap<(String, String), Vec<&JobRow>> = HashMap::new();
    for row in &rows {
        if row.status == "done" && row.output_path.is_some() {
            tuples
                .entry((row.tier.clone(), row.channel.clone()))
                .or_default()
                .push(row);
        }
    }

    let primary_str = primary.to_string();
    let total_sources = absorbed.len() + 1;

    let mut out = Vec::with_capacity(tuples.len());
    for ((tier, channel), trip_rows) in tuples {
        let trip_set: HashSet<&str> =
            trip_rows.iter().map(|r| r.trip_id.as_str()).collect();
        let primary_has = trip_set.contains(primary_str.as_str());
        let absorbed_with_output: Vec<String> = absorbed
            .iter()
            .map(|u| u.to_string())
            .filter(|s| trip_set.contains(s.as_str()))
            .collect();
        let coverage = trip_set.len();
        let status = if coverage == total_sources {
            TupleStatus::Concatenable
        } else {
            TupleStatus::PartialOutputs
        };
        out.push(TupleAssessment {
            tier,
            channel,
            status,
            primary_has,
            absorbed_with_output,
        });
    }

    // Stable order for the dialog (tier first, then channel).
    out.sort_by(|a, b| {
        a.tier.cmp(&b.tier).then_with(|| a.channel.cmp(&b.channel))
    });

    Ok(TimelapseMergeAssessment {
        has_any_timelapses: true,
        tuples: out,
        camera_kinds,
    })
}

fn load_camera_kinds(
    conn: &Connection,
    trip_ids: &[String],
) -> Result<Vec<String>, AppError> {
    if trip_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", trip_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT camera_kind FROM trips WHERE id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(trip_ids.iter()), |r| {
        r.get::<_, String>(0)
    })?;
    let mut kinds = Vec::new();
    for r in rows {
        kinds.push(r?);
    }
    kinds.sort();
    kinds.dedup();
    Ok(kinds)
}

// ── Merge ──────────────────────────────────────────────────────────

pub fn merge_trips(
    db: &DbHandle,
    primary: Uuid,
    absorbed: &[Uuid],
    strategy: TimelapseMergeStrategy,
) -> Result<MergeReport, AppError> {
    if absorbed.is_empty() {
        return Err(AppError::Internal(
            "merge_trips called with empty absorbed list".into(),
        ));
    }
    if absorbed.contains(&primary) {
        return Err(AppError::Internal(
            "primary trip cannot also appear in absorbed list".into(),
        ));
    }

    let primary_str = primary.to_string();
    let absorbed_strs: Vec<String> =
        absorbed.iter().map(|u| u.to_string()).collect();

    // Phase 1: handle timelapse outputs (concat or delete). Done
    // outside the main DB transaction because ffmpeg invocations are
    // long-running and must not hold a write lock; the resulting row
    // mutations are applied in their own short transactions.
    let mut report = MergeReport {
        primary_trip_id: primary_str.clone(),
        absorbed_trip_ids: absorbed_strs.clone(),
        ..MergeReport::default()
    };

    let mut all_ids = absorbed_strs.clone();
    all_ids.push(primary_str.clone());

    // Snapshot job rows under a short-lived lock so we can release it
    // before invoking ffmpeg.
    let job_rows = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        load_job_rows(&conn, &all_ids)?
    };

    let removed_during_timelapse =
        apply_timelapse_strategy(db, &primary_str, &absorbed_strs, &job_rows, strategy, &mut report)?;
    report.timelapse_jobs_removed = removed_during_timelapse;

    // Phase 2: rewrite segments + tags + trips, and record the merge
    // directive, all in one transaction.
    {
        let mut conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let tx = conn.transaction()?;

        rewrite_trip_id_columns(&tx, &primary_str, &absorbed_strs)?;
        rebuild_primary_trip_row(&tx, &primary_str)?;
        delete_absorbed_trip_rows(&tx, &absorbed_strs)?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        for absorbed_id in absorbed {
            crate::db::manual_trip_merges::insert_merge(
                &tx, primary, *absorbed_id, now_ms,
            )?;
        }

        tx.commit()?;
    }

    Ok(report)
}

fn apply_timelapse_strategy(
    db: &DbHandle,
    primary: &str,
    absorbed: &[String],
    job_rows: &[JobRow],
    strategy: TimelapseMergeStrategy,
    report: &mut MergeReport,
) -> Result<usize, AppError> {
    if job_rows.is_empty() {
        return Ok(0);
    }

    match strategy {
        TimelapseMergeStrategy::DiscardAll => {
            // Delete every timelapse_jobs row across primary + absorbed
            // in one shot. Don't touch the MP4 files on disk — they
            // become orphans but disk-space cost is bounded and a
            // future cleanup can sweep them.
            let mut all_ids = absorbed.to_vec();
            all_ids.push(primary.to_string());
            let mut conn = db
                .lock()
                .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
            let tx = conn.transaction()?;
            let placeholders = std::iter::repeat_n("?", all_ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM timelapse_jobs WHERE trip_id IN ({placeholders})"
            );
            let n = tx.execute(&sql, rusqlite::params_from_iter(all_ids.iter()))?;
            tx.commit()?;
            Ok(n)
        }
        TimelapseMergeStrategy::ConcatWherePossible => {
            apply_concat_where_possible(db, primary, absorbed, job_rows, report)
        }
    }
}

fn apply_concat_where_possible(
    db: &DbHandle,
    primary: &str,
    absorbed: &[String],
    job_rows: &[JobRow],
    report: &mut MergeReport,
) -> Result<usize, AppError> {
    // Group done rows by (tier, channel). Non-done rows are dropped
    // entirely (no useful output to carry forward).
    let mut by_tuple: HashMap<(String, String), Vec<&JobRow>> = HashMap::new();
    let mut non_done_count = 0usize;
    for row in job_rows {
        if row.status == "done" && row.output_path.is_some() {
            by_tuple
                .entry((row.tier.clone(), row.channel.clone()))
                .or_default()
                .push(row);
        } else {
            non_done_count += 1;
        }
    }

    // ffmpeg path + library root for the concat output filename.
    let (ffmpeg_path, library_root) = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let path = crate::db::settings::get(&conn, SETTING_FFMPEG_PATH)?
            .ok_or_else(|| {
                AppError::Internal(
                    "ffmpeg not configured — set the path in Timelapse settings first".into(),
                )
            })?;
        let lib = crate::db::settings::get(&conn, SETTING_LIBRARY_ROOT)?
            .ok_or_else(|| {
                AppError::Internal(
                    "library root not yet known — open a folder once before merging".into(),
                )
            })?;
        (path, lib)
    };

    let timelapses_dir = PathBuf::from(&library_root).join("Timelapses");
    let total_sources = absorbed.len() + 1;
    let mut concatenated_paths_to_keep: HashSet<String> = HashSet::new();
    let mut rows_to_delete: Vec<(String, String, String)> = Vec::new();
    let mut rows_to_upsert: Vec<UpsertJob> = Vec::new();

    for ((tier, channel), trip_rows) in &by_tuple {
        let trip_set: HashSet<&str> =
            trip_rows.iter().map(|r| r.trip_id.as_str()).collect();

        if trip_set.len() == total_sources {
            // Every source has it — concat is feasible.
            // Order rows by source: primary first, then absorbed in input order.
            let mut ordered: Vec<&JobRow> = Vec::with_capacity(trip_set.len());
            for trip_row in trip_rows.iter() {
                if trip_row.trip_id == primary {
                    ordered.push(trip_row);
                }
            }
            for absorbed_id in absorbed {
                if let Some(r) = trip_rows.iter().find(|r| r.trip_id == *absorbed_id) {
                    ordered.push(*r);
                }
            }

            let merged_output = timelapses_dir
                .join(format!("{primary}_{tier}_{channel}.mp4"));
            let merged_path_str = merged_output.to_string_lossy().to_string();

            match concat_outputs(&ffmpeg_path, &ordered, &merged_output) {
                Ok(()) => {
                    let curve_json = merge_speed_curves(&ordered);
                    let padded_count: i64 =
                        ordered.iter().map(|r| r.padded_count).sum();
                    let encoder_used = ordered
                        .iter()
                        .find_map(|r| r.encoder_used.clone());
                    let ffmpeg_version = ordered
                        .iter()
                        .find_map(|r| r.ffmpeg_version.clone());
                    rows_to_upsert.push(UpsertJob {
                        trip_id: primary.to_string(),
                        tier: tier.clone(),
                        channel: channel.clone(),
                        output_path: merged_path_str.clone(),
                        speed_curve_json: curve_json,
                        padded_count,
                        encoder_used,
                        ffmpeg_version,
                    });
                    concatenated_paths_to_keep.insert(merged_path_str);
                    report.concatenated.push((tier.clone(), channel.clone()));

                    // Source rows for this tuple are replaced by the
                    // merged row above. Primary's on-disk file IS the
                    // merged output (concat_outputs renamed the temp
                    // file into the canonical primary path); absorbed
                    // files become orphans for a future cleanup sweep.
                    for trip_row in &ordered {
                        rows_to_delete.push((
                            trip_row.trip_id.clone(),
                            tier.clone(),
                            channel.clone(),
                        ));
                    }
                }
                Err(e) => {
                    // Leave rows untouched. Phase 2's
                    // rewrite_trip_id_columns dedupe step will drop
                    // absorbed rows for this tuple because primary
                    // still has its row, so the merged trip keeps
                    // primary's existing timelapse intact rather than
                    // silently losing both the file and the row.
                    eprintln!(
                        "[merge] concat failed for ({tier}, {channel}): {e}; \
                         keeping primary's existing output"
                    );
                }
            }
        } else {
            // Partial coverage. Drop all rows for this tuple — the
            // merged trip will have nothing for it; the user clicks
            // Rebuild to get a fresh encode.
            for trip_row in trip_rows {
                rows_to_delete.push((
                    trip_row.trip_id.clone(),
                    tier.clone(),
                    channel.clone(),
                ));
            }
        }
        let _ = total_sources; // for IDE clarity; loop scope only.
    }

    // Apply DB changes in a single transaction.
    let mut deleted_count = 0;
    {
        let mut conn = db
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        let tx = conn.transaction()?;
        for (trip_id, tier, channel) in &rows_to_delete {
            let n = tx.execute(
                "DELETE FROM timelapse_jobs
                 WHERE trip_id = ?1 AND tier = ?2 AND channel = ?3",
                params![trip_id, tier, channel],
            )?;
            deleted_count += n;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        for j in &rows_to_upsert {
            tx.execute(
                "INSERT INTO timelapse_jobs
                    (trip_id, tier, channel, status, output_path,
                     ffmpeg_version, encoder_used, padded_count,
                     speed_curve_json, created_at_ms, completed_at_ms)
                 VALUES (?1, ?2, ?3, 'done', ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(trip_id, tier, channel) DO UPDATE SET
                    status = excluded.status,
                    output_path = excluded.output_path,
                    ffmpeg_version = excluded.ffmpeg_version,
                    encoder_used = excluded.encoder_used,
                    padded_count = excluded.padded_count,
                    speed_curve_json = excluded.speed_curve_json,
                    completed_at_ms = excluded.completed_at_ms",
                params![
                    j.trip_id,
                    j.tier,
                    j.channel,
                    j.output_path,
                    j.ffmpeg_version,
                    j.encoder_used,
                    j.padded_count,
                    j.speed_curve_json,
                    now_ms,
                ],
            )?;
        }
        tx.commit()?;
    }

    let _ = non_done_count;
    let _ = concatenated_paths_to_keep;
    Ok(deleted_count)
}

struct UpsertJob {
    trip_id: String,
    tier: String,
    channel: String,
    output_path: String,
    speed_curve_json: String,
    padded_count: i64,
    encoder_used: Option<String>,
    ffmpeg_version: Option<String>,
}

/// Run ffmpeg's concat demuxer to splice the given input MP4s into
/// `output`. Lossless (no re-encode). The caller is responsible for
/// matching codec/resolution/fps across inputs — concat will fail
/// loudly if they differ.
fn concat_outputs(
    ffmpeg_path: &str,
    inputs: &[&JobRow],
    output: &std::path::Path,
) -> Result<(), AppError> {
    if inputs.is_empty() {
        return Err(AppError::Internal("concat called with no inputs".into()));
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // The merged output's canonical path collides with primary's
    // existing timelapse — both follow the
    // `<library_root>/Timelapses/<trip_id>_<tier>_<channel>.mp4`
    // convention and the merged trip's id IS the primary's id. Writing
    // ffmpeg's output directly to `output` would clobber an input
    // before it's read. Stage to a sibling temp file and rename into
    // place only after a successful encode; on failure or partial
    // write, primary's existing file is preserved so the row deletion
    // path can keep its existing output.
    let temp_output = {
        let stem = output
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("merged");
        output.with_file_name(format!(".{stem}.merging.mp4"))
    };
    let _ = std::fs::remove_file(&temp_output);

    // Build the concat list file in the parent directory so relative
    // paths resolve correctly. Use absolute paths to be safe.
    let list_path = output.with_extension("concat.txt");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&list_path)?;
        for row in inputs {
            // Each input must exist or concat fails fast — surface
            // missing-file errors early with a clear message.
            let p = row.output_path.as_deref().ok_or_else(|| {
                AppError::Internal(format!(
                    "input {tier}/{channel} from trip {trip} has no output_path",
                    tier = row.tier,
                    channel = row.channel,
                    trip = row.trip_id,
                ))
            })?;
            if !std::path::Path::new(p).exists() {
                return Err(AppError::Internal(format!(
                    "concat input does not exist on disk: {p}"
                )));
            }
            // The concat demuxer's mini-format wants single-quoted
            // paths. Single quotes inside paths must be escaped — but
            // Windows paths shouldn't contain them, so we keep this
            // simple.
            writeln!(f, "file '{}'", p.replace('\'', "'\\''"))?;
        }
    }

    let result = ffmpeg_command(ffmpeg_path)
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        .arg(&temp_output)
        .output()
        .map_err(|e| AppError::Internal(format!("failed to spawn ffmpeg: {e}")))?;

    let _ = std::fs::remove_file(&list_path);

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let _ = std::fs::remove_file(&temp_output);
        return Err(AppError::Internal(format!(
            "ffmpeg concat failed: {stderr}"
        )));
    }

    // Promote temp → final. Remove the existing primary file first so
    // rename works portably (Windows rename refuses an existing target
    // on older Rust toolchains, and explicit remove is unambiguous).
    let _ = std::fs::remove_file(output);
    std::fs::rename(&temp_output, output).map_err(|e| {
        let _ = std::fs::remove_file(&temp_output);
        AppError::Internal(format!(
            "merged output rename failed: {e}"
        ))
    })?;
    Ok(())
}

/// Concatenate the per-trip speed curves into a single curve covering
/// the merged trip's concat-time. Each successive curve is shifted by
/// the accumulated concat-end of the prior one.
fn merge_speed_curves(inputs: &[&JobRow]) -> String {
    let mut merged: Vec<CurveSegment> = Vec::new();
    let mut offset = 0.0;
    for row in inputs {
        let Some(json) = row.speed_curve_json.as_deref() else {
            continue;
        };
        let Some(parsed) =
            crate::timelapse::speed_curve::deserialize_curve(json)
        else {
            continue;
        };
        if parsed.is_empty() {
            continue;
        }
        let local_max = parsed
            .iter()
            .map(|s| s.concat_end)
            .fold(0.0_f64, f64::max);
        for s in parsed {
            merged.push(CurveSegment {
                concat_start: s.concat_start + offset,
                concat_end: s.concat_end + offset,
                rate: s.rate,
            });
        }
        offset += local_max;
    }
    crate::timelapse::speed_curve::serialize_curve(&merged)
}

fn rewrite_trip_id_columns(
    tx: &Connection,
    primary: &str,
    absorbed: &[String],
) -> Result<(), AppError> {
    if absorbed.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", absorbed.len())
        .collect::<Vec<_>>()
        .join(",");

    // Order matters: tags + segments first (cheap), then any leftover
    // timelapse_jobs (the strategy step usually deletes/upserts these
    // already; this catches stragglers like pending or failed rows
    // for tuples whose status was 'pending' or 'failed').
    for table in &["tags", "segments"] {
        let sql = format!(
            "UPDATE {table} SET trip_id = ? WHERE trip_id IN ({placeholders})"
        );
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&primary];
        for a in absorbed {
            params_vec.push(a as &dyn rusqlite::ToSql);
        }
        tx.execute(&sql, rusqlite::params_from_iter(params_vec))?;
    }

    // timelapse_jobs PK is (trip_id, tier, channel); a naive UPDATE
    // could collide if the primary already has a row for the same
    // (tier, channel). Resolve by deleting the absorbed rows for
    // tuples the primary already covers, then updating the rest.
    let dedupe_sql = format!(
        "DELETE FROM timelapse_jobs
         WHERE trip_id IN ({placeholders})
           AND (tier, channel) IN
               (SELECT tier, channel FROM timelapse_jobs WHERE trip_id = ?)"
    );
    let mut dedupe_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
    for a in absorbed {
        dedupe_params.push(a as &dyn rusqlite::ToSql);
    }
    dedupe_params.push(&primary);
    tx.execute(&dedupe_sql, rusqlite::params_from_iter(dedupe_params))?;

    let upd_sql = format!(
        "UPDATE timelapse_jobs SET trip_id = ? WHERE trip_id IN ({placeholders})"
    );
    let mut upd_params: Vec<&dyn rusqlite::ToSql> = vec![&primary];
    for a in absorbed {
        upd_params.push(a as &dyn rusqlite::ToSql);
    }
    tx.execute(&upd_sql, rusqlite::params_from_iter(upd_params))?;

    Ok(())
}

fn rebuild_primary_trip_row(tx: &Connection, primary: &str) -> Result<(), AppError> {
    // Recompute span from the segments now under primary. If primary
    // has no segments (rare — archive-only post-merge), leave the row
    // alone; persist_and_gc will keep it via the timelapse_jobs guard.
    let span: Option<(i64, i64)> = tx
        .query_row(
            "SELECT MIN(start_time_ms),
                    MAX(start_time_ms + CAST(duration_s * 1000 AS INTEGER))
             FROM segments WHERE trip_id = ?1",
            params![primary],
            |r| {
                let a: Option<i64> = r.get(0)?;
                let b: Option<i64> = r.get(1)?;
                Ok(a.zip(b))
            },
        )?;
    if let Some((start_ms, end_ms)) = span {
        tx.execute(
            "UPDATE trips SET start_time_ms = ?1, end_time_ms = ?2 WHERE id = ?3",
            params![start_ms, end_ms, primary],
        )?;
    }
    Ok(())
}

fn delete_absorbed_trip_rows(tx: &Connection, absorbed: &[String]) -> Result<(), AppError> {
    if absorbed.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", absorbed.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql =
        format!("DELETE FROM trips WHERE id IN ({placeholders})");
    tx.execute(&sql, rusqlite::params_from_iter(absorbed.iter()))?;
    Ok(())
}
