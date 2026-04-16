//! Group parsed dashcam files into segments and trips.
//!
//! A segment is a set of channel files all recorded at roughly the same
//! time (same `group_key`, timestamps within a fuzzy window). We accept
//! any channel count from 1 to N — the old Wolf-Box-only version required
//! exactly three (F/I/R), which blocked users of 2-channel and 4-channel
//! dashcams.

use crate::model::{label_rank, Channel, Segment, Trip};
use crate::scan::naming::{EventMode, ParsedName};
use chrono::{Duration, NaiveDateTime};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use uuid::Uuid;

pub const DEFAULT_TRIP_GAP_SECONDS: i64 = 120;
pub const ASSUMED_SEGMENT_DURATION_S: f64 = 180.0;
/// Across a multi-channel segment the per-file timestamps can drift by
/// up to a couple of seconds (empirical, Wolf Box). Any two files with
/// the same `group_key` are already considered a segment, but if two
/// segments share a key within this window we merge them.
pub const SEGMENT_FUZZY_WINDOW_S: i64 = 3;

#[derive(Debug, Clone)]
pub struct GroupingInput {
    pub path: PathBuf,
    pub parsed: ParsedName,
}

#[derive(Debug, Clone)]
pub struct GroupingOutput {
    pub trips: Vec<Trip>,
    pub unmatched: Vec<String>,
}

pub fn group(items: Vec<GroupingInput>, trip_gap_s: i64) -> GroupingOutput {
    // Bucket by group_key — every parser is responsible for producing a
    // key that uniquely identifies a recording instance.
    let mut buckets: HashMap<String, Vec<GroupingInput>> = HashMap::new();
    for item in items {
        buckets
            .entry(item.parsed.group_key.clone())
            .or_default()
            .push(item);
    }

    // Each bucket becomes one segment. No minimum channel count.
    let mut segments: Vec<Segment> = Vec::with_capacity(buckets.len());
    for (_, bucket) in buckets {
        segments.push(make_segment(bucket));
    }

    // Merge segments whose start times are within SEGMENT_FUZZY_WINDOW_S
    // and share an event_mode. This catches cases where two parsers
    // generated slightly different group_keys for what should be one segment
    // (e.g. filename clock skew pushing across a second boundary).
    segments.sort_by_key(|s| s.start_time);
    let segments = merge_fuzzy_neighbors(segments);

    let trips = merge_into_trips(segments, trip_gap_s);
    GroupingOutput {
        trips,
        // With the group-key approach, a file either parses (and becomes
        // a 1+ channel segment) or doesn't parse at all (and is already
        // in `errors` upstream). There's no more "unmatched" category.
        unmatched: Vec::new(),
    }
}

fn make_segment(bucket: Vec<GroupingInput>) -> Segment {
    // Use the earliest timestamp in the bucket as the segment start.
    // Event mode comes from any file (they all share group_key, so they
    // all share event_mode by construction).
    let start_time = bucket
        .iter()
        .map(|i| i.parsed.start_time)
        .min()
        .expect("bucket is non-empty by construction");
    let event_mode = bucket[0].parsed.event_mode;
    // Camera kind is a per-parser property. All files in a bucket come
    // from the same group_key and thus the same parser, so this is stable.
    let camera_kind = bucket[0].parsed.camera_kind;

    let mut channels: Vec<Channel> = bucket.into_iter().map(make_channel).collect();
    // Canonical order: Front, Interior, Rear, then others alphabetically.
    channels.sort_by(|a, b| label_rank(&a.label).cmp(&label_rank(&b.label)));

    // Drop duplicate labels — if two files with the same parser label
    // show up in one bucket, keep the first after canonical sort.
    channels.dedup_by(|a, b| a.label == b.label);

    Segment {
        id: Uuid::new_v4(),
        start_time,
        duration_s: 0.0,
        is_event: matches!(event_mode, EventMode::Event),
        channels,
        gps_supported: camera_kind.gps_supported(),
        camera_kind,
    }
}

fn make_channel(item: GroupingInput) -> Channel {
    Channel {
        label: item.parsed.channel_label,
        file_path: item.path.to_string_lossy().into_owned(),
        width: None,
        height: None,
        fps_num: None,
        fps_den: None,
        codec: None,
        has_gpmd_track: false,
    }
}

/// Merge any adjacent segments whose start times are within
/// `SEGMENT_FUZZY_WINDOW_S`, share `is_event`, **and have no overlapping
/// channel labels**. This handles timestamp skew across channels from the
/// same camera (Wolf Box's per-channel timestamps can drift by 1s) while
/// preserving independent segments from different cameras that happen to
/// record at the same moment (mixed-format folders).
fn merge_fuzzy_neighbors(segments: Vec<Segment>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::with_capacity(segments.len());
    for seg in segments {
        if let Some(last) = out.last_mut() {
            let delta = (seg.start_time - last.start_time).num_seconds().abs();
            let within_window = delta <= SEGMENT_FUZZY_WINDOW_S;
            let same_event_mode = last.is_event == seg.is_event;

            let last_labels: HashSet<&str> =
                last.channels.iter().map(|c| c.label.as_str()).collect();
            let disjoint = seg
                .channels
                .iter()
                .all(|c| !last_labels.contains(c.label.as_str()));

            if within_window && same_event_mode && disjoint {
                let mut combined: Vec<Channel> = last.channels.drain(..).collect();
                combined.extend(seg.channels);
                combined.sort_by(|a, b| label_rank(&a.label).cmp(&label_rank(&b.label)));
                last.channels = combined;
                continue;
            }
        }
        out.push(seg);
    }
    out
}

fn merge_into_trips(segments: Vec<Segment>, trip_gap_s: i64) -> Vec<Trip> {
    let mut trips: Vec<Trip> = Vec::new();
    let mut current: Vec<Segment> = Vec::new();
    let mut current_end: Option<NaiveDateTime> = None;

    for seg in segments {
        let seg_start = seg.start_time;
        let duration = if seg.duration_s > 0.0 {
            seg.duration_s
        } else {
            ASSUMED_SEGMENT_DURATION_S
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
        ASSUMED_SEGMENT_DURATION_S
    };
    let end_time = last.start_time + Duration::seconds(last_duration as i64);
    Trip {
        id: Uuid::new_v4(),
        start_time,
        end_time,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LABEL_FRONT, LABEL_INTERIOR, LABEL_REAR};
    use crate::scan::naming;

    fn input(name: &str) -> GroupingInput {
        GroupingInput {
            path: PathBuf::from(format!("E:\\fake\\{name}")),
            parsed: naming::parse(name).unwrap(),
        }
    }

    #[test]
    fn wolf_box_triplet_makes_one_segment_and_trip() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 3);
        // Canonical order: Front, Interior, Rear.
        assert_eq!(out.trips[0].segments[0].channels[0].label, LABEL_FRONT);
        assert_eq!(out.trips[0].segments[0].channels[1].label, LABEL_INTERIOR);
        assert_eq!(out.trips[0].segments[0].channels[2].label, LABEL_REAR);
    }

    #[test]
    fn thinkware_pair_makes_two_channel_segment() {
        let items = vec![
            input("REC_2026_03_06_07_25_52_F.MP4"),
            input("REC_2026_03_06_07_25_52_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 2);
        assert_eq!(out.trips[0].segments[0].channels[0].label, LABEL_FRONT);
        assert_eq!(out.trips[0].segments[0].channels[1].label, LABEL_REAR);
    }

    #[test]
    fn single_file_becomes_one_channel_segment() {
        // No more "unmatched" for partially-recorded segments.
        let items = vec![input("2026_03_23_094634_00_F.MP4")];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 1);
        assert!(out.unmatched.is_empty());
    }

    #[test]
    fn four_channel_segment_groups_together() {
        let items = vec![
            input("2026_03_06_072552_A.MP4"),
            input("2026_03_06_072552_B.MP4"),
            input("2026_03_06_072552_C.MP4"),
            input("2026_03_06_072552_D.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 4);
    }

    #[test]
    fn event_flag_propagates() {
        let items = vec![
            input("2026_03_15_173951_02_F.MP4"),
            input("2026_03_15_173951_02_I.MP4"),
            input("2026_03_15_173951_02_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert!(out.trips[0].segments[0].is_event);
    }

    #[test]
    fn consecutive_segments_merge_into_one_trip() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
            input("2026_03_23_094934_00_F.MP4"),
            input("2026_03_23_094934_00_I.MP4"),
            input("2026_03_23_094934_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments.len(), 2);
    }

    #[test]
    fn large_gap_splits_into_separate_trips() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
            input("2026_03_23_114634_00_F.MP4"),
            input("2026_03_23_114634_00_I.MP4"),
            input("2026_03_23_114634_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 2);
    }

    #[test]
    fn segments_sorted_by_start_time_within_trip() {
        let items = vec![
            input("2026_03_23_094934_00_F.MP4"),
            input("2026_03_23_094934_00_I.MP4"),
            input("2026_03_23_094934_00_R.MP4"),
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        let segs = &out.trips[0].segments;
        assert!(segs[0].start_time < segs[1].start_time);
    }

    #[test]
    fn skew_across_event_boundary_does_not_merge_different_event_modes() {
        // Normal front and event I/R are different recordings; must not merge.
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094635_02_I.MP4"),
            input("2026_03_23_094635_02_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        // Two segments: one 1-channel (Normal Front), one 2-channel (Event I+R).
        let total_segments: usize = out.trips.iter().map(|t| t.segments.len()).sum();
        assert_eq!(total_segments, 2);
    }

    #[test]
    fn mixed_dashcam_formats_group_separately() {
        // Wolf Box and Thinkware files in the same folder shouldn't cross-pollinate.
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
            input("REC_2026_03_23_09_46_34_F.MP4"),
            input("REC_2026_03_23_09_46_34_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        let all_segments: Vec<&Segment> =
            out.trips.iter().flat_map(|t| t.segments.iter()).collect();
        assert_eq!(all_segments.len(), 2);
        // The two segments have different channel counts; make sure we
        // didn't merge them.
        let counts: Vec<usize> = all_segments.iter().map(|s| s.channels.len()).collect();
        assert!(counts.contains(&3));
        assert!(counts.contains(&2));
    }
}
