//! Per-trip × per-scan coverage matrix for the Scan view's Trips
//! section. Joins `segments` × `scan_runs` and contrasts each row's
//! recorded `version` against the registered scan's current version
//! to derive a four-bucket tally per (trip, scan):
//! done-current / stale / failed / not-run.
//!
//! The frontend turns the bucket counts into a single pill state
//! (✓ done · ⚠ stale · ◐ partial · ✗ failed · ○ not run). Returning
//! the raw counts rather than the derived state lets the UI surface
//! tooltip detail like "8/12 segments · 2 stale" without a second
//! round-trip.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::db::DbHandle;
use crate::error::AppError;
use crate::scans::registry;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCoverage {
    pub scan_id: String,
    pub total_segments: u32,
    pub done_count: u32,
    pub stale_count: u32,
    pub failed_count: u32,
    pub not_run_count: u32,
    /// Up to 3 distinct `error_message` strings from failed runs for
    /// this (trip, scan). Empty when `failed_count == 0`. Used by the
    /// tooltip to explain *why* a pill is red without a second query.
    /// Capped to keep payload size sane on libraries with many trips.
    pub sample_failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TripScanCoverage {
    pub trip_id: String,
    pub per_scan: Vec<ScanCoverage>,
}

pub fn list_scan_coverage(db: &DbHandle) -> Result<Vec<TripScanCoverage>, AppError> {
    let registry = registry();
    let current_versions: HashMap<String, u32> = registry
        .iter()
        .map(|s| (s.id().to_string(), s.version()))
        .collect();
    let scan_order: Vec<String> = registry.iter().map(|s| s.id().to_string()).collect();

    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;

    // Walk segments once, building both the per-trip total of present
    // files AND a set of segment IDs whose master file is missing on
    // disk. A missing file isn't a scannable unit (File::open would
    // error with NotFound) so it shouldn't appear in the total or in
    // any failed-count tally. Without this, a single deleted MP4
    // makes the trip read "9/10 done · 1 failed" forever even though
    // the missing file is just gone.
    //
    // Tombstones (`is_tombstone = 1`) are intentionally non-scannable —
    // they're already excluded from the total and shouldn't be counted
    // as "missing" failures either, so we filter them out at the SQL
    // level rather than after-the-fact.
    let mut totals: HashMap<String, u32> = HashMap::new();
    let mut missing_segments: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, trip_id, master_path FROM segments WHERE is_tombstone = 0",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            let (seg_id, trip_id, master_path) = r?;
            if Path::new(&master_path).exists() {
                *totals.entry(trip_id).or_insert(0) += 1;
            } else {
                missing_segments.insert(seg_id);
            }
        }
    }

    // Bucket scan_runs by (trip_id, scan_id) into (done_current, stale, failed).
    // INNER JOIN — segments without any scan_runs simply don't appear, and the
    // not-run bucket is computed below by subtraction from the trip's total.
    // Rows belonging to missing-file segments are skipped: stale error rows
    // recorded before the worker started pre-flighting file existence
    // shouldn't keep showing the trip as "1 failed."
    let mut buckets: HashMap<(String, String), (u32, u32, u32)> = HashMap::new();
    let mut sample_failures: HashMap<(String, String), Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT s.id, s.trip_id, sr.scan_id, sr.status, sr.version, sr.error_message
             FROM segments s
             INNER JOIN scan_runs sr ON s.id = sr.segment_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? as u32,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        for r in rows {
            let (segment_id, trip_id, scan_id, status, version, error_message) = r?;
            if missing_segments.contains(&segment_id) {
                continue;
            }
            let key = (trip_id, scan_id.clone());
            let entry = buckets.entry(key.clone()).or_insert((0, 0, 0));
            // worker.rs writes "ok" on success, "error" on failure. Anything
            // else is treated as failed for safety (better to surface it as
            // red than silently drop it from the tally).
            if status == "ok" {
                let current = current_versions.get(&scan_id).copied().unwrap_or(0);
                if version >= current {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            } else {
                entry.2 += 1;
                if let Some(msg) = error_message {
                    let bucket = sample_failures.entry(key).or_default();
                    if bucket.len() < 3 && !bucket.contains(&msg) {
                        bucket.push(msg);
                    }
                }
            }
        }
    }
    drop(conn);

    let mut trip_ids: Vec<String> = totals.keys().cloned().collect();
    trip_ids.sort();

    let mut out = Vec::with_capacity(trip_ids.len());
    for trip_id in trip_ids {
        let total = totals.get(&trip_id).copied().unwrap_or(0);
        let mut per_scan = Vec::with_capacity(scan_order.len());
        for scan_id in &scan_order {
            let key = (trip_id.clone(), scan_id.clone());
            let (done, stale, failed) =
                buckets.get(&key).copied().unwrap_or((0, 0, 0));
            let not_run = total.saturating_sub(done + stale + failed);
            let failures = sample_failures.get(&key).cloned().unwrap_or_default();
            per_scan.push(ScanCoverage {
                scan_id: scan_id.clone(),
                total_segments: total,
                done_count: done,
                stale_count: stale,
                failed_count: failed,
                not_run_count: not_run,
                sample_failures: failures,
            });
        }
        out.push(TripScanCoverage { trip_id, per_scan });
    }
    Ok(out)
}
