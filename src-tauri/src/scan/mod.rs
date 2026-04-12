pub mod grouping;
pub mod naming;
pub mod walker;

use crate::error::AppError;
use crate::metadata::mp4_probe;
use crate::model::{ChannelKind, ScanError, ScanResult, Segment, Trip};
use crate::scan::grouping::{GroupingInput, DEFAULT_TRIP_GAP_SECONDS};
use chrono::{Duration, NaiveDateTime};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[tauri::command]
pub async fn scan_folder(path: String) -> Result<ScanResult, AppError> {
    scan_folder_sync(Path::new(&path))
}

pub fn scan_folder_sync(root: &Path) -> Result<ScanResult, AppError> {
    if !root.is_dir() {
        return Err(AppError::Internal(format!(
            "not a directory: {}",
            root.display()
        )));
    }

    let files = walker::find_mp4_files(root);
    eprintln!(
        "scan_folder: found {} mp4 files under {}",
        files.len(),
        root.display()
    );

    // Stage 1: parse filenames. Bad names → scan errors.
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
            Err(e) => errors.push(ScanError {
                path: file.to_string_lossy().into_owned(),
                reason: e.to_string(),
            }),
        }
    }

    // Stage 2: assemble triplets (with stub durations for now).
    let group_out = grouping::group(parsed_inputs, DEFAULT_TRIP_GAP_SECONDS);
    let mut trips = group_out.trips;
    let unmatched = group_out.unmatched;

    // Stage 3: parallel probe every channel file → fill metadata + real durations.
    // Collect paths per segment (immutable borrow of trips, dropped before stage 4).
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
        .zip(
            trips
                .iter()
                .flat_map(|t| t.segments.iter())
                .map(|s| s.channels.iter().map(|c| c.kind).collect::<Vec<_>>())
                .collect::<Vec<_>>()
                .par_iter(),
        )
        .map(|(paths, kinds)| probe_segment(paths, kinds))
        .collect();

    // Apply probe outcomes to segments (mutable borrow now that immutable is dropped).
    {
        let seg_iter = trips.iter_mut().flat_map(|t| t.segments.iter_mut());
        for (seg, probe) in seg_iter.zip(probe_outcomes.iter()) {
            if let Some(d) = probe.front_duration {
                seg.duration_s = d;
            }
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

    // Stage 4: re-run trip merging with real durations. The stub 180s assumption
    // may have mis-merged event segments that were shorter.
    let all_segments: Vec<Segment> = trips.into_iter().flat_map(|t| t.segments).collect();
    let final_trips = remerge_trips(all_segments, DEFAULT_TRIP_GAP_SECONDS);

    Ok(ScanResult {
        trips: final_trips,
        unmatched,
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
    front_duration: Option<f64>,
    channels: Vec<ProbedChannel>,
    errors: Vec<ScanError>,
}

fn probe_segment(paths: &[PathBuf], kinds: &[ChannelKind]) -> SegmentProbe {
    let mut out = SegmentProbe::default();
    for (path, kind) in paths.iter().zip(kinds.iter()) {
        match mp4_probe::probe(path) {
            Ok(meta) => {
                if *kind == ChannelKind::Front {
                    out.front_duration = Some(meta.duration_s);
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
                out.errors.push(ScanError {
                    path: path.to_string_lossy().into_owned(),
                    reason: e.to_string(),
                });
                out.channels.push(ProbedChannel::default());
            }
        }
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
    let start_time = segments.first().unwrap().start_time;
    let last = segments.last().unwrap();
    let last_duration = if last.duration_s > 0.0 {
        last.duration_s
    } else {
        180.0
    };
    let end_time = last.start_time + Duration::seconds(last_duration as i64);
    Trip {
        id: Uuid::new_v4(),
        start_time,
        end_time,
        segments,
    }
}
