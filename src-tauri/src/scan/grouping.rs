//! Group parsed dashcam files into segments and trips.
//!
//! A segment is a set of channel files all recorded at roughly the same
//! time (same `group_key`, timestamps within a fuzzy window). We accept
//! any channel count from 1 to N — the old Wolf-Box-only version required
//! exactly three (F/I/R), which blocked users of 2-channel and 4-channel
//! dashcams.

use crate::error::AppError;
use crate::model::{label_rank, Channel, Segment, Trip};
use crate::scan::naming::{self, EventMode, ParsedName};
use chrono::{Duration, NaiveDateTime};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
}

pub fn group(
    items: Vec<GroupingInput>,
    trip_gap_s: i64,
    archive_root: &std::path::Path,
) -> GroupingOutput {
    // Bucket by group_key — every parser is responsible for producing a
    // key that uniquely identifies a recording instance.
    let mut buckets: HashMap<String, Vec<GroupingInput>> = HashMap::new();
    for mut item in items {
        item.parsed = super::viofo::apply_path_context(item.parsed, &item.path);
        buckets
            .entry(item.parsed.group_key.clone())
            .or_default()
            .push(item);
    }

    // Each bucket becomes one segment. No minimum channel count. With the
    // group-key approach, a file either parses (and becomes a 1+ channel
    // segment) or doesn't parse at all (and is already in `errors`
    // upstream) — there is no separate "unmatched" category anymore.
    let mut segments = Vec::with_capacity(buckets.len());
    for (_, bucket) in buckets {
        let mode = bucket[0].parsed.event_mode;
        segments.push((make_segment(bucket, archive_root), mode));
    }

    // Merge segments whose start times are within SEGMENT_FUZZY_WINDOW_S
    // and share an event_mode. This catches cases where two parsers
    // generated slightly different group_keys for what should be one segment
    // (e.g. filename clock skew pushing across a second boundary).
    segments.sort_by_key(|(s, _)| s.start_time);
    let segments = merge_fuzzy_neighbors(segments);

    let trips = merge_into_trips(segments, trip_gap_s);
    GroupingOutput { trips }
}

fn make_segment(bucket: Vec<GroupingInput>, archive_root: &std::path::Path) -> Segment {
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
    channels.sort_by_key(|c| label_rank(&c.label));

    // Drop duplicate labels — if two files with the same parser label
    // show up in one bucket, keep the first after canonical sort.
    channels.dedup_by(|a, b| a.label == b.label);

    // Derive the segment UUID from the master channel's *archive-relative*
    // path so the same file produces the same UUID regardless of which
    // OS or mount point the archive is opened on. Falls back to the
    // absolute path when the file lives outside the active archive (a
    // scan-folder-of-arbitrary-directory case): the resulting UUID is
    // OS-specific but at least stable within that scan.
    let master_abs = std::path::Path::new(&channels[0].file_path);
    let id_key = match crate::paths::to_archive_relative(master_abs, archive_root) {
        Ok(rel) => rel,
        Err(_) => channels[0].file_path.clone(),
    };
    let id = crate::model::derive_segment_id(&id_key, start_time);
    Segment {
        id,
        start_time,
        duration_s: 0.0,
        is_event: matches!(event_mode, EventMode::Event | EventMode::ParkingEvent),
        channels,
        gps_supported: camera_kind.gps_supported(),
        camera_kind,
        size_bytes: None,
        is_tombstone: false,
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

/// Merge compatible segments whose start times are within
/// `SEGMENT_FUZZY_WINDOW_S`, share `is_event`, **and have no overlapping
/// channel labels**. VIOFO additionally retains recording mode throughout the
/// window search. This handles timestamp skew across channels from the
/// same camera (Wolf Box's per-channel timestamps can drift by 1s) while
/// preserving independent segments from different cameras that happen to
/// record at the same moment (mixed-format folders).
fn merge_fuzzy_neighbors(segments: Vec<(Segment, EventMode)>) -> Vec<Segment> {
    let mut out: Vec<(Segment, EventMode)> = Vec::with_capacity(segments.len());
    for (seg, mode) in segments {
        let compatible = |(last, last_mode): &(Segment, EventMode)| {
            let within_window =
                (seg.start_time - last.start_time).num_seconds().abs() <= SEGMENT_FUZZY_WINDOW_S;
            let same_event_mode = last.is_event == seg.is_event;
            let same_recording_mode =
                seg.camera_kind != naming::CameraKind::Viofo || *last_mode == mode;
            let same_camera = last.camera_kind == seg.camera_kind;
            let last_labels: HashSet<&str> =
                last.channels.iter().map(|c| c.label.as_str()).collect();
            let disjoint = seg
                .channels
                .iter()
                .all(|c| !last_labels.contains(c.label.as_str()));
            within_window && same_camera && same_event_mode && same_recording_mode && disjoint
        };
        // VIOFO modes may interleave within the skew window. An intervening
        // parking/event segment must not prevent a normal F/R pair matching.
        // Retain the existing adjacent-only policy for other camera formats.
        let target = if seg.camera_kind == naming::CameraKind::Viofo {
            (0..out.len())
                .rev()
                .take_while(|index| {
                    (seg.start_time - out[*index].0.start_time)
                        .num_seconds()
                        .abs()
                        <= SEGMENT_FUZZY_WINDOW_S
                })
                .find(|index| compatible(&out[*index]))
        } else {
            out.len()
                .checked_sub(1)
                .filter(|index| compatible(&out[*index]))
        };
        if let Some(index) = target {
            let last = &mut out[index].0;
            last.channels.extend(seg.channels);
            last.channels.sort_by_key(|c| label_rank(&c.label));
        } else {
            out.push((seg, mode));
        }
    }
    out.into_iter().map(|(segment, _)| segment).collect()
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
    let start_time = segments
        .first()
        .expect("close_trip on non-empty")
        .start_time;
    let last = segments.last().expect("close_trip on non-empty");
    let last_duration = if last.duration_s > 0.0 {
        last.duration_s
    } else {
        ASSUMED_SEGMENT_DURATION_S
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

/// Find the sibling file for a given front-channel file in its parent
/// directory.
///
/// Mirrors the scanner's Stage 1 (`group_key` bucketing) + Stage 2
/// (`merge_fuzzy_neighbors`) matching — but for the common "I have one
/// front file, find its rear" case used by the timelapse encoder. We
/// *cannot* just swap the channel letter in the filename: Wolf Box
/// routinely records per-channel timestamps skewed by 1-2 seconds
/// (the scanner tolerates this via `SEGMENT_FUZZY_WINDOW_S`), and a
/// naive string swap produces paths that don't exist when skew is
/// present.
///
/// Returns the candidate with the smallest time delta within the
/// fuzzy window, or `None` if nothing qualifies. Deterministic: same
/// input → same output across runs.
pub fn find_sibling_file(
    front_path: &Path,
    target_channel_label: &str,
) -> Result<Option<PathBuf>, AppError> {
    let Some(parent) = front_path.parent() else {
        return Ok(None);
    };
    let listing = read_and_parse_listing(parent)?;
    find_sibling_in_listing(front_path, target_channel_label, &listing)
}

/// One pre-parsed entry from a directory listing. `parsed` is the
/// successful output of `naming::parse(filename)`; filenames the
/// parser rejects are dropped at listing-build time so consumers
/// don't re-evaluate them. Cheap to clone for cross-call caches.
#[derive(Debug, Clone)]
pub struct ListingEntry {
    pub path: PathBuf,
    pub parsed: naming::ParsedName,
}

/// Read a single directory, parsing each filename and discarding
/// the entries the naming parser rejects. Used by the timelapse
/// pre-flight probe to amortize one `read_dir` + N `parse` calls
/// across many sibling lookups. Errors propagate (unreadable
/// directory should fail the operation, not silently no-match).
pub fn read_and_parse_listing(parent: &Path) -> Result<Vec<ListingEntry>, AppError> {
    let entries = std::fs::read_dir(parent).map_err(AppError::Io)?;
    let mut out: Vec<ListingEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(parsed) = naming::parse_path(&path) else {
            continue;
        };
        out.push(ListingEntry { path, parsed });
    }
    Ok(out)
}

/// Match the same fuzzy-window sibling rules as `find_sibling_file`
/// but against a pre-built listing instead of re-reading the dir.
/// Caller is responsible for ensuring the listing covers the same
/// directory that `front_path` lives in — `find_sibling_file` is
/// the convenience wrapper that ties the two together.
pub fn find_sibling_in_listing(
    front_path: &Path,
    target_channel_label: &str,
    listing: &[ListingEntry],
) -> Result<Option<PathBuf>, AppError> {
    let _front_name = match front_path.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return Ok(None),
    };
    let Ok(anchor) = naming::parse_path(front_path) else {
        return Ok(None);
    };

    let mut best: Option<(i64, PathBuf)> = None;
    // Note: don't skip when `entry.path == front_path`. When the
    // caller is asking for a channel that *equals* the anchor's
    // channel (e.g. the timelapse worker's Front job probing a
    // Front master for its own F sibling), the master itself IS
    // the sibling — returning None there would force a
    // false-negative placeholder. For different-channel lookups
    // the channel-label filter below excludes the anchor anyway.
    for entry in listing {
        if entry.parsed.camera_kind != anchor.camera_kind {
            continue;
        }
        if entry.parsed.channel_label != target_channel_label {
            continue;
        }
        if entry.parsed.event_mode != anchor.event_mode {
            continue;
        }
        let delta = (entry.parsed.start_time - anchor.start_time)
            .num_seconds()
            .abs();
        if delta > SEGMENT_FUZZY_WINDOW_S {
            continue;
        }
        if best.as_ref().map(|(d, _)| delta < *d).unwrap_or(true) {
            best = Some((delta, entry.path.clone()));
        }
    }
    Ok(best.map(|(_, p)| p))
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
    fn viofo_fuzzy_grouping_preserves_normal_parking_and_ro_events() {
        let mut normal = input("2020_0101_120000_F.MP4");
        normal.path = PathBuf::from("/source/DCIM/2020_0101_120000_F.MP4");
        let mut event = input("2020_0101_120000_R.MP4");
        event.path = PathBuf::from("/source/RO/2020_0101_120000_R.MP4");
        let parking = input("2020_0101_120001_PI.MP4");
        let output = group(vec![normal, event, parking], 120, Path::new("/source"));
        let segments: Vec<_> = output.trips.iter().flat_map(|t| &t.segments).collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments.iter().filter(|s| s.is_event).count(), 1);
        assert!(segments.iter().all(|s| s.channels.len() == 1));
    }

    #[test]
    fn viofo_same_mode_skew_merges_across_minute_boundary() {
        for (directory, marker, is_event) in [
            ("DCIM", "", false),
            ("DCIM", "P", false),
            ("RO", "", true),
            ("RO", "P", true),
        ] {
            let mut front = input(&format!("2020_0101_120059_{marker}F.MP4"));
            let mut rear = input(&format!("2020_0101_120100_{marker}R.MP4"));
            for item in [&mut front, &mut rear] {
                let filename = item
                    .path
                    .to_string_lossy()
                    .rsplit('\\')
                    .next()
                    .unwrap()
                    .to_owned();
                item.path = PathBuf::from("/source").join(directory).join(filename);
            }
            let output = group(vec![front, rear], 120, Path::new("/source"));
            let segments: Vec<_> = output.trips.iter().flat_map(|t| &t.segments).collect();
            assert_eq!(segments.len(), 1);
            assert_eq!(segments[0].channels.len(), 2);
            assert_eq!(segments[0].is_event, is_event);
        }
    }

    #[test]
    fn viofo_interleaved_recording_modes_do_not_hide_a_matching_channel() {
        let output = group(
            vec![
                input("2020_0101_120000_F.MP4"),
                input("2020_0101_120001_PI.MP4"),
                input("2020_0101_120002_R.MP4"),
            ],
            120,
            Path::new("/source"),
        );
        let segments: Vec<_> = output.trips.iter().flat_map(|t| &t.segments).collect();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].channels.len(), 2);
        assert_eq!(segments[1].channels.len(), 1);
    }

    #[test]
    fn viofo_sibling_lookup_keeps_protected_parking_and_driving_separate() {
        let root = tempfile::tempdir().unwrap();
        let ro = root.path().join("RO");
        std::fs::create_dir(&ro).unwrap();
        for name in [
            "2020_0101_120000_F.MP4",
            "2020_0101_120000_PF.MP4",
            "2020_0101_120001_PR.MP4",
            "2020_0101_120002_R.MP4",
        ] {
            std::fs::write(ro.join(name), []).unwrap();
        }
        assert_eq!(
            find_sibling_file(&ro.join("2020_0101_120000_F.MP4"), LABEL_REAR).unwrap(),
            Some(ro.join("2020_0101_120002_R.MP4"))
        );
        assert_eq!(
            find_sibling_file(&ro.join("2020_0101_120000_PF.MP4"), LABEL_REAR).unwrap(),
            Some(ro.join("2020_0101_120001_PR.MP4"))
        );
    }

    #[test]
    fn viofo_never_fuzzy_merges_with_another_camera_kind() {
        let output = group(
            vec![
                input("2020_0101_120000_F.MP4"),
                input("2020_01_01_120001_00_R.MP4"),
            ],
            120,
            Path::new("/source"),
        );
        let segments: Vec<_> = output.trips.iter().flat_map(|t| &t.segments).collect();
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|s| s.channels.len() == 1));
    }

    #[test]
    fn wolf_box_triplet_makes_one_segment_and_trip() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
        ];
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 1);
    }

    #[test]
    fn four_channel_segment_groups_together() {
        let items = vec![
            input("2026_03_06_072552_A.MP4"),
            input("2026_03_06_072552_B.MP4"),
            input("2026_03_06_072552_C.MP4"),
            input("2026_03_06_072552_D.MP4"),
        ];
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
        let out = group(
            items,
            DEFAULT_TRIP_GAP_SECONDS,
            std::path::Path::new("/tmp/tripviewer-test-archive"),
        );
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
