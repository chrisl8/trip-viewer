pub mod errors;
pub mod grouping;
pub mod naming;
pub mod walker;

use crate::error::AppError;
use crate::metadata::mp4_probe;
use crate::model::{ScanError, ScanResult, Segment, Trip};
use crate::scan::errors::classify;
use crate::scan::grouping::{GroupingInput, DEFAULT_TRIP_GAP_SECONDS};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Best-effort read of file size and last-modified time. Returns `(None, None)`
/// if the path is gone or unreadable — these fields are decorative, not load-bearing.
fn file_stats(path: &Path) -> (Option<u64>, Option<i64>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (None, None);
    };
    let size = Some(meta.len());
    let modified = meta
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|dt| dt.timestamp_millis());
    (size, modified)
}

fn make_scan_error(path: &Path, err: &AppError) -> ScanError {
    let (size_bytes, modified_ms) = file_stats(path);
    let c = classify(err);
    ScanError {
        path: path.to_string_lossy().into_owned(),
        kind: c.kind,
        message: c.message,
        detail: c.detail,
        size_bytes,
        modified_ms,
    }
}

#[tauri::command]
pub async fn scan_folder(
    path: String,
    db: tauri::State<'_, crate::db::DbHandle>,
) -> Result<ScanResult, AppError> {
    let mut result = scan_folder_sync(Path::new(&path))?;
    let scan_started_ms = chrono::Utc::now().timestamp_millis();
    // Persistence is best-effort; a DB failure must not block the user
    // from seeing their scan results, they just won't have tags yet.
    // We swap in the merge-applied trip list so the frontend renders
    // the same view the DB now has: natural trips that match a
    // `manual_trip_merges` directive get folded into their primary.
    if let Ok(mut conn) = db.lock() {
        match crate::db::segments::persist_and_gc(&mut conn, &result.trips, scan_started_ms) {
            Ok(merged) => result.trips = merged,
            Err(e) => eprintln!("[db] persist_and_gc failed: {e}"),
        }
    }
    Ok(result)
}

pub fn scan_folder_sync(root: &Path) -> Result<ScanResult, AppError> {
    if !root.is_dir() {
        return Err(AppError::Internal(format!(
            "not a directory: {}",
            root.display()
        )));
    }

    let files = walker::find_video_files(root);
    eprintln!(
        "scan_folder: found {} video files under {}",
        files.len(),
        root.display()
    );

    // Stage 1: parse filenames. Files we can't parse go to scan errors.
    let mut parsed_inputs: Vec<GroupingInput> = Vec::with_capacity(files.len());
    let mut errors: Vec<ScanError> = Vec::new();
    for file in files {
        let name = match file.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        match naming::parse(name) {
            Ok(parsed) => parsed_inputs.push(GroupingInput {
                path: file,
                parsed,
            }),
            Err(e) => errors.push(make_scan_error(&file, &e)),
        }
    }

    // Stage 2: group parsed files into segments and trips. Any channel
    // count (1–N) is accepted.
    let group_out = grouping::group(parsed_inputs, DEFAULT_TRIP_GAP_SECONDS);
    let mut trips = group_out.trips;

    // Stage 3: parallel probe every channel file → fill metadata + real
    // durations. The master (first channel in canonical order) provides
    // the segment duration.
    let segment_paths: Vec<Vec<PathBuf>> = trips
        .iter()
        .flat_map(|t| t.segments.iter())
        .map(|seg| {
            seg.channels
                .iter()
                .map(|c| PathBuf::from(&c.file_path))
                .collect()
        })
        .collect();

    let probe_outcomes: Vec<SegmentProbe> = segment_paths
        .par_iter()
        .map(|paths| probe_segment(paths))
        .collect();

    // Apply probe outcomes to segments.
    {
        let seg_iter = trips.iter_mut().flat_map(|t| t.segments.iter_mut());
        for (seg, probe) in seg_iter.zip(probe_outcomes.iter()) {
            if let Some(d) = probe.master_duration {
                seg.duration_s = d;
            }
            seg.size_bytes = probe.size_bytes;
            for (ch, pch) in seg.channels.iter_mut().zip(probe.channels.iter()) {
                ch.width = pch.width;
                ch.height = pch.height;
                ch.fps_num = pch.fps_num;
                ch.fps_den = pch.fps_den;
                ch.codec = pch.codec.clone();
                ch.has_gpmd_track = pch.has_gpmd_track;
            }
        }
    }
    for probe in probe_outcomes {
        errors.extend(probe.errors);
    }

    // Stage 4: re-run trip merging with real durations. The stub 180s
    // assumption may have mis-merged short event segments.
    let all_segments: Vec<Segment> = trips.into_iter().flat_map(|t| t.segments).collect();
    let final_trips = remerge_trips(all_segments, DEFAULT_TRIP_GAP_SECONDS);

    Ok(ScanResult {
        trips: final_trips,
        errors,
    })
}

#[derive(Debug, Clone, Default)]
struct ProbedChannel {
    width: Option<u32>,
    height: Option<u32>,
    fps_num: Option<u32>,
    fps_den: Option<u32>,
    codec: Option<String>,
    has_gpmd_track: bool,
}

#[derive(Debug, Default)]
struct SegmentProbe {
    /// Duration of the master channel (first in canonical order).
    master_duration: Option<f64>,
    channels: Vec<ProbedChannel>,
    errors: Vec<ScanError>,
    /// Sum of `fs::metadata.len()` across every channel file in the
    /// segment. `None` only if every file failed to stat.
    size_bytes: Option<u64>,
}

fn probe_segment(paths: &[PathBuf]) -> SegmentProbe {
    let mut out = SegmentProbe::default();
    let mut size_total: u64 = 0;
    let mut any_size = false;
    for (idx, path) in paths.iter().enumerate() {
        if let Ok(meta) = std::fs::metadata(path) {
            size_total = size_total.saturating_add(meta.len());
            any_size = true;
        }
        match mp4_probe::probe(path) {
            Ok(meta) => {
                if idx == 0 {
                    out.master_duration = Some(meta.duration_s);
                }
                out.channels.push(ProbedChannel {
                    width: Some(meta.width),
                    height: Some(meta.height),
                    fps_num: Some(meta.fps_num),
                    fps_den: Some(meta.fps_den),
                    codec: Some(meta.codec),
                    has_gpmd_track: meta.has_gpmd_track,
                });
            }
            Err(e) => {
                out.errors.push(make_scan_error(path, &e));
                out.channels.push(ProbedChannel::default());
            }
        }
    }
    if any_size {
        out.size_bytes = Some(size_total);
    }
    out
}

fn remerge_trips(segments: Vec<Segment>, trip_gap_s: i64) -> Vec<Trip> {
    let mut segments = segments;
    segments.sort_by_key(|s| s.start_time);

    let mut trips: Vec<Trip> = Vec::new();
    let mut current: Vec<Segment> = Vec::new();
    let mut current_end: Option<NaiveDateTime> = None;

    for seg in segments {
        let seg_start = seg.start_time;
        let duration = if seg.duration_s > 0.0 {
            seg.duration_s
        } else {
            180.0
        };
        let seg_end = seg_start + Duration::seconds(duration as i64);

        match current_end {
            None => {
                current.push(seg);
                current_end = Some(seg_end);
            }
            Some(prev_end) => {
                let gap = (seg_start - prev_end).num_seconds();
                if gap <= trip_gap_s {
                    current.push(seg);
                    current_end = Some(seg_end);
                } else {
                    trips.push(close_trip(std::mem::take(&mut current)));
                    current.push(seg);
                    current_end = Some(seg_end);
                }
            }
        }
    }
    if !current.is_empty() {
        trips.push(close_trip(current));
    }
    trips
}

fn close_trip(segments: Vec<Segment>) -> Trip {
    let start_time = segments.first().expect("close_trip on non-empty").start_time;
    let last = segments.last().expect("close_trip on non-empty");
    let last_duration = if last.duration_s > 0.0 {
        last.duration_s
    } else {
        180.0
    };
    let end_time = last.start_time + Duration::seconds(last_duration as i64);
    let id = crate::model::derive_trip_id(segments[0].id);
    let camera_kind = segments[0].camera_kind;
    let gps_supported = segments[0].gps_supported;
    Trip {
        id,
        start_time,
        end_time,
        segments,
        camera_kind,
        gps_supported,
        archive_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use std::fs;

    #[test]
    fn probe_segment_sums_channel_file_sizes() {
        // Two scratch files with known sizes. mp4_probe will fail on
        // these (they're not real MP4s) and the channel records will
        // be empty defaults — that's fine; we're only asserting the
        // size aggregation path which runs independently of mp4_probe.
        let dir = temp_dir().join(format!(
            "tripviewer-probe-size-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p1 = dir.join("a.bin");
        let p2 = dir.join("b.bin");
        fs::write(&p1, vec![0u8; 1_024]).unwrap();
        fs::write(&p2, vec![0u8; 2_048]).unwrap();

        let probe = probe_segment(&[p1.clone(), p2.clone()]);
        assert_eq!(probe.size_bytes, Some(3_072));

        // A path that doesn't exist is silently skipped — total only
        // reflects the files that stat'd successfully.
        let p_missing = dir.join("ghost.bin");
        let probe = probe_segment(&[p1.clone(), p_missing]);
        assert_eq!(probe.size_bytes, Some(1_024));

        // All paths missing → size_bytes is None, not Some(0). The
        // distinction lets the UI render "—" instead of misleadingly
        // claiming the trip is zero bytes.
        let only_missing = dir.join("nope.bin");
        let probe = probe_segment(&[only_missing]);
        assert!(probe.size_bytes.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
