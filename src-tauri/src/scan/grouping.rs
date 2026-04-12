use crate::model::{Channel, ChannelKind, Segment, Trip};
use crate::scan::naming::{EventMode, ParsedName};
use chrono::{Duration, NaiveDateTime};
use std::path::PathBuf;
use uuid::Uuid;

pub const DEFAULT_TRIP_GAP_SECONDS: i64 = 120;
pub const ASSUMED_SEGMENT_DURATION_S: f64 = 180.0;
/// Wolf Box records each channel with a per-file timestamp. Across a triplet
/// the three filenames can drift by up to 1 second (empirical), so match
/// fuzzily within this window.
pub const TRIPLET_FUZZY_WINDOW_S: i64 = 3;

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
    let mut fronts: Vec<GroupingInput> = Vec::new();
    let mut interiors: Vec<Option<GroupingInput>> = Vec::new();
    let mut rears: Vec<Option<GroupingInput>> = Vec::new();

    for item in items {
        match item.parsed.channel {
            ChannelKind::Front => fronts.push(item),
            ChannelKind::Interior => interiors.push(Some(item)),
            ChannelKind::Rear => rears.push(Some(item)),
        }
    }

    fronts.sort_by_key(|i| i.parsed.start_time);
    interiors.sort_by_key(|slot| {
        slot.as_ref()
            .map(|i| i.parsed.start_time)
            .unwrap_or_default()
    });
    rears.sort_by_key(|slot| {
        slot.as_ref()
            .map(|i| i.parsed.start_time)
            .unwrap_or_default()
    });

    let mut segments: Vec<Segment> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    for front in fronts {
        let front_t = front.parsed.start_time;
        let front_event = front.parsed.event_mode;

        let i_idx = find_match(&interiors, front_t, front_event, TRIPLET_FUZZY_WINDOW_S);
        let r_idx = find_match(&rears, front_t, front_event, TRIPLET_FUZZY_WINDOW_S);

        match (i_idx, r_idx) {
            (Some(i), Some(r)) => {
                let interior = interiors[i].take().expect("claim interior");
                let rear = rears[r].take().expect("claim rear");
                segments.push(Segment {
                    id: Uuid::new_v4(),
                    start_time: front_t,
                    duration_s: 0.0,
                    is_event: matches!(front_event, EventMode::Event),
                    channels: vec![
                        make_channel(&front),
                        make_channel(&interior),
                        make_channel(&rear),
                    ],
                });
            }
            _ => unmatched.push(front.path.to_string_lossy().into_owned()),
        }
    }

    for slot in interiors.into_iter().flatten() {
        unmatched.push(slot.path.to_string_lossy().into_owned());
    }
    for slot in rears.into_iter().flatten() {
        unmatched.push(slot.path.to_string_lossy().into_owned());
    }

    segments.sort_by_key(|s| s.start_time);
    let trips = merge_into_trips(segments, trip_gap_s);
    GroupingOutput { trips, unmatched }
}

fn find_match(
    list: &[Option<GroupingInput>],
    target_t: NaiveDateTime,
    event_mode: EventMode,
    window_s: i64,
) -> Option<usize> {
    let mut best: Option<(usize, i64)> = None;
    for (idx, slot) in list.iter().enumerate() {
        let Some(item) = slot else { continue };
        if item.parsed.event_mode != event_mode {
            continue;
        }
        let delta_s = (item.parsed.start_time - target_t).num_seconds();
        if delta_s.abs() <= window_s {
            match best {
                None => best = Some((idx, delta_s.abs())),
                Some((_, prev_abs)) if delta_s.abs() < prev_abs => {
                    best = Some((idx, delta_s.abs()))
                }
                _ => {}
            }
        } else if delta_s > window_s {
            // List is sorted ascending by start_time; nothing further will match.
            break;
        }
    }
    best.map(|(idx, _)| idx)
}

fn make_channel(item: &GroupingInput) -> Channel {
    Channel {
        kind: item.parsed.channel,
        file_path: item.path.to_string_lossy().into_owned(),
        width: None,
        height: None,
        fps_num: None,
        fps_den: None,
        codec: None,
        has_gpmd_track: false,
    }
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
    let start_time = segments.first().unwrap().start_time;
    let last = segments.last().unwrap();
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
    use crate::scan::naming;

    fn input(name: &str) -> GroupingInput {
        GroupingInput {
            path: PathBuf::from(format!("E:\\fake\\{name}")),
            parsed: naming::parse(name).unwrap(),
        }
    }

    #[test]
    fn single_complete_triplet_makes_one_segment_and_trip() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
            input("2026_03_23_094634_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments.len(), 1);
        assert_eq!(out.trips[0].segments[0].channels.len(), 3);
        assert!(out.unmatched.is_empty());
    }

    #[test]
    fn missing_channel_goes_to_unmatched() {
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094634_00_I.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert!(out.trips.is_empty());
        assert_eq!(out.unmatched.len(), 2);
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
    fn rear_clock_skew_within_window_still_matches() {
        // Event triplet with rear channel off by 1 second.
        let items = vec![
            input("2026_03_22_164128_02_F.MP4"),
            input("2026_03_22_164128_02_I.MP4"),
            input("2026_03_22_164127_02_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert_eq!(out.trips.len(), 1);
        assert_eq!(out.trips[0].segments.len(), 1);
        assert!(out.unmatched.is_empty());
    }

    #[test]
    fn skew_across_event_boundary_does_not_match_different_event_modes() {
        // A front _00_ at t=100 must not pair with an interior _02_ at t=101.
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094635_02_I.MP4"),
            input("2026_03_23_094635_02_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        assert!(out.trips.is_empty());
        assert_eq!(out.unmatched.len(), 3);
    }

    #[test]
    fn fuzzy_match_picks_closest_candidate() {
        // Two fronts, two interiors — make sure greedy picks the closest pairing.
        let items = vec![
            input("2026_03_23_094634_00_F.MP4"),
            input("2026_03_23_094636_00_I.MP4"),
            input("2026_03_23_094636_00_R.MP4"),
            input("2026_03_23_094637_00_F.MP4"),
            input("2026_03_23_094639_00_I.MP4"),
            input("2026_03_23_094639_00_R.MP4"),
        ];
        let out = group(items, DEFAULT_TRIP_GAP_SECONDS);
        // Both fronts should match their closest interior/rear.
        assert_eq!(
            out.trips
                .iter()
                .flat_map(|t| t.segments.iter())
                .count(),
            2
        );
        assert!(out.unmatched.is_empty());
    }
}
