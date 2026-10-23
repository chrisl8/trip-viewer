//! Turns a user's export request (trip segments, time range, layout
//! choices) into a concrete plan: which segments to read, which channels
//! become ffmpeg inputs and in what order, which audio to use, and where
//! the GPS for the mini-map comes from.
//!
//! Pure and Tauri-free so the selection rules can be unit-tested.

use serde::Deserialize;

use crate::error::AppError;
use crate::export::layout::{ExportCodec, ExportQuality, LayoutKind, PipEdge, SlotSize};
use crate::model::label_rank;
use crate::scan::naming::CameraKind;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub label: String,
    pub path: String,
}

/// One segment of the trip, with the file of every channel it has.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSegment {
    pub duration_s: f64,
    pub files: Vec<ExportFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAudio {
    pub label: String,
    /// 1.0 = unchanged, 0.5 = half, 2.0 = double.
    pub volume: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartExportArgs {
    pub output_path: String,
    pub segments: Vec<ExportSegment>,
    pub camera_kind: CameraKind,
    /// Range within the trip (seconds from the start of the first segment).
    pub start_s: f64,
    pub duration_s: f64,
    pub layout: LayoutKind,
    pub slot_size: SlotSize,
    pub pip_edge: PipEdge,
    pub quality: ExportQuality,
    /// Missing = H.264, so older callers keep working.
    #[serde(default)]
    pub codec: ExportCodec,
    pub main_label: String,
    /// Which other channels to show small, in order. `None` = all of them.
    pub slot_labels: Option<Vec<String>>,
    pub show_map: bool,
    /// Missing = route only.
    #[serde(default)]
    pub map_style: MapStyle,
    /// Empty = silent export.
    pub audio: Vec<ExportAudio>,
}

/// What the GPS mini-map is drawn on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MapStyle {
    /// Route and dot on a plain dark background. Works offline.
    #[default]
    Route,
    /// Same, over OpenStreetMap tiles downloaded for this export.
    Streets,
}

pub const MAX_VOLUME: f64 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub struct GpsSource {
    pub path: String,
    /// Where this segment's t=0 falls relative to the start of the clip.
    pub offset_s: f64,
    pub duration_s: f64,
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub local_start_s: f64,
    pub duration_s: f64,
    /// Channel labels in ffmpeg input order: main, small tiles, then any
    /// channel used only for its audio.
    pub labels: Vec<String>,
    pub main: usize,
    pub slots: Vec<usize>,
    pub audio: Vec<(usize, f64)>,
    /// Concat-demuxer list text, one per entry of `labels`.
    pub list_texts: Vec<String>,
    /// Files to read GPS from (the camera's master channel per segment).
    pub gps_sources: Vec<GpsSource>,
    pub warnings: Vec<String>,
}

/// Concat-demuxer list. Paths are single-quoted with embedded quotes
/// escaped. The explicit `duration` gives every channel the same offset
/// at each segment boundary: channels of one segment can differ by a
/// fraction of a second in their own length (audio vs video tracks), and
/// without it the channels would drift apart over a long trip.
pub fn concat_list_text(entries: &[(&str, f64)]) -> String {
    let mut s = String::new();
    for (path, dur) in entries {
        s.push_str("file '");
        s.push_str(&path.replace('\'', "'\\''"));
        s.push_str("'\n");
        s.push_str(&format!("duration {dur:.3}\n"));
    }
    s
}

fn err(msg: impl Into<String>) -> AppError {
    AppError::Internal(msg.into())
}

pub fn prepare(args: &StartExportArgs) -> Result<Prepared, AppError> {
    if args.segments.is_empty() {
        return Err(err("nothing to export: the trip has no segments"));
    }
    if !(args.start_s.is_finite() && args.duration_s.is_finite()) || args.start_s < 0.0 {
        return Err(err("invalid export range"));
    }
    if args.duration_s < 1.0 {
        return Err(err("the clip must be at least 1 second long"));
    }
    if args
        .segments
        .iter()
        .any(|s| !s.duration_s.is_finite() || s.duration_s <= 0.0)
    {
        return Err(err("a segment has no duration"));
    }

    // Segment start offsets within the trip.
    let mut starts = Vec::with_capacity(args.segments.len());
    let mut total = 0.0;
    for s in &args.segments {
        starts.push(total);
        total += s.duration_s;
    }
    if args.start_s >= total {
        return Err(err("the clip starts after the end of the trip"));
    }
    let duration_s = args.duration_s.min(total - args.start_s);
    let end_s = args.start_s + duration_s;

    // Segments that overlap [start, end).
    let first = (0..args.segments.len())
        .find(|&i| starts[i] + args.segments[i].duration_s > args.start_s)
        .ok_or_else(|| err("no segment covers the start of the clip"))?;
    let last = (first..args.segments.len())
        .take_while(|&i| starts[i] < end_s)
        .last()
        .unwrap_or(first);
    let selected = &args.segments[first..=last];
    let local_start_s = args.start_s - starts[first];

    // Channels present in every selected segment. A channel missing from
    // some segment (camera off for a while) can't be kept in sync, so it
    // is dropped with a warning instead of failing the whole export.
    let mut warnings = Vec::new();
    let mut all_labels: Vec<String> = Vec::new();
    for s in selected {
        for f in &s.files {
            if !all_labels.contains(&f.label) {
                all_labels.push(f.label.clone());
            }
        }
    }
    all_labels.sort_by_key(|l| label_rank(l));
    let available: Vec<String> = all_labels
        .iter()
        .filter(|l| {
            selected
                .iter()
                .all(|s| s.files.iter().any(|f| &f.label == *l))
        })
        .cloned()
        .collect();
    for l in &all_labels {
        if !available.contains(l) {
            warnings.push(format!(
                "{l} skipped: it is missing from some of the selected segments"
            ));
        }
    }

    if !available.contains(&args.main_label) {
        return Err(err(format!(
            "main channel '{}' is not available in the selected segments",
            args.main_label
        )));
    }

    let mut labels = vec![args.main_label.clone()];
    let wanted: Vec<String> = match &args.slot_labels {
        Some(v) => v.clone(),
        None => available.clone(),
    };
    for l in wanted {
        if l == args.main_label || labels.contains(&l) {
            continue;
        }
        if available.contains(&l) {
            labels.push(l);
        } else if !warnings
            .iter()
            .any(|w| w.starts_with(&format!("{l} skipped")))
        {
            warnings.push(format!("{l} skipped: no such channel in this clip"));
        }
    }
    let slots: Vec<usize> = (1..labels.len()).collect();

    // Audio sources may be channels that are not shown on screen.
    let mut audio = Vec::new();
    for a in &args.audio {
        if !(a.volume.is_finite() && (0.0..=MAX_VOLUME).contains(&a.volume)) {
            return Err(err(format!("invalid volume for {}", a.label)));
        }
        if !available.contains(&a.label) {
            return Err(err(format!(
                "audio source '{}' is not available in the selected segments",
                a.label
            )));
        }
        let idx = match labels.iter().position(|l| l == &a.label) {
            Some(i) => i,
            None => {
                labels.push(a.label.clone());
                labels.len() - 1
            }
        };
        if !audio.iter().any(|&(i, _)| i == idx) {
            audio.push((idx, a.volume));
        }
    }

    let list_texts: Vec<String> = labels
        .iter()
        .map(|label| {
            let entries: Vec<(&str, f64)> = selected
                .iter()
                .map(|s| {
                    let f = s
                        .files
                        .iter()
                        .find(|f| &f.label == label)
                        .expect("label checked present in every selected segment");
                    (f.path.as_str(), s.duration_s)
                })
                .collect();
            concat_list_text(&entries)
        })
        .collect();

    // GPS lives in the camera's master channel (first in canonical order).
    let gps_label = &available[0];
    let gps_sources = selected
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let f = s.files.iter().find(|f| &f.label == gps_label).unwrap();
            GpsSource {
                path: f.path.clone(),
                offset_s: starts[first + k] - args.start_s,
                duration_s: s.duration_s,
            }
        })
        .collect();

    Ok(Prepared {
        local_start_s,
        duration_s,
        labels,
        main: 0,
        slots,
        audio,
        list_texts,
        gps_sources,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(dur: f64, files: &[(&str, &str)]) -> ExportSegment {
        ExportSegment {
            duration_s: dur,
            files: files
                .iter()
                .map(|(l, p)| ExportFile {
                    label: l.to_string(),
                    path: p.to_string(),
                })
                .collect(),
        }
    }

    fn t800_segments(n: usize) -> Vec<ExportSegment> {
        (0..n)
            .map(|i| {
                seg(
                    60.0,
                    &[
                        ("Rear", &format!("/r/{i}R.MP4")),
                        ("Front", &format!("/f/{i}F.MP4")),
                        ("Interior", &format!("/c/{i}C.MP4")),
                    ],
                )
            })
            .collect()
    }

    fn args(segments: Vec<ExportSegment>) -> StartExportArgs {
        StartExportArgs {
            output_path: "/o/out.mp4".into(),
            segments,
            camera_kind: CameraKind::Generic,
            start_s: 0.0,
            duration_s: 30.0,
            layout: LayoutKind::Side,
            slot_size: SlotSize::Medium,
            pip_edge: PipEdge::Bottom,
            quality: ExportQuality::Balanced,
            codec: ExportCodec::H264,
            main_label: "Front".into(),
            slot_labels: None,
            show_map: true,
            map_style: MapStyle::Route,
            audio: vec![ExportAudio {
                label: "Front".into(),
                volume: 1.0,
            }],
        }
    }

    #[test]
    fn concat_list_escapes_quotes_and_sets_duration() {
        let t = concat_list_text(&[("/a/it's.mp4", 60.0), ("/b.mp4", 59.5)]);
        assert_eq!(
            t,
            "file '/a/it'\\''s.mp4'\nduration 60.000\nfile '/b.mp4'\nduration 59.500\n"
        );
    }

    #[test]
    fn default_order_is_main_then_canonical_others() {
        let p = prepare(&args(t800_segments(3))).unwrap();
        assert_eq!(p.labels, vec!["Front", "Interior", "Rear"]);
        assert_eq!(p.main, 0);
        assert_eq!(p.slots, vec![1, 2]);
        assert_eq!(p.audio, vec![(0, 1.0)]);
    }

    #[test]
    fn only_overlapping_segments_are_used() {
        let mut a = args(t800_segments(5));
        a.start_s = 130.0; // inside segment 2 (120..180)
        a.duration_s = 30.0;
        let p = prepare(&a).unwrap();
        assert!((p.local_start_s - 10.0).abs() < 1e-9);
        assert_eq!(p.gps_sources.len(), 1);
        assert_eq!(p.gps_sources[0].path, "/f/2F.MP4");
        assert!((p.gps_sources[0].offset_s - (-10.0)).abs() < 1e-9);
        assert_eq!(p.list_texts[0].matches("file '").count(), 1);
    }

    #[test]
    fn clip_spanning_a_boundary_uses_both_segments() {
        let mut a = args(t800_segments(5));
        a.start_s = 50.0;
        a.duration_s = 30.0; // 50..80 crosses 60
        let p = prepare(&a).unwrap();
        assert_eq!(p.gps_sources.len(), 2);
        assert!((p.local_start_s - 50.0).abs() < 1e-9);
        assert!((p.gps_sources[1].offset_s - 10.0).abs() < 1e-9);
        assert!(p.list_texts[1].contains("/c/1C.MP4"));
    }

    #[test]
    fn range_is_clamped_to_the_end_of_the_trip() {
        let mut a = args(t800_segments(2));
        a.start_s = 100.0;
        a.duration_s = 500.0;
        let p = prepare(&a).unwrap();
        assert!((p.duration_s - 20.0).abs() < 1e-9);
    }

    #[test]
    fn start_after_the_end_is_an_error() {
        let mut a = args(t800_segments(2));
        a.start_s = 500.0;
        assert!(prepare(&a).is_err());
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        let mut a = args(t800_segments(2));
        a.duration_s = 0.2;
        assert!(prepare(&a).is_err());
        let mut a = args(t800_segments(2));
        a.start_s = -1.0;
        assert!(prepare(&a).is_err());
        let mut a = args(t800_segments(2));
        a.duration_s = f64::NAN;
        assert!(prepare(&a).is_err());
        assert!(prepare(&args(vec![])).is_err());
    }

    #[test]
    fn channel_missing_from_a_segment_is_dropped_with_a_warning() {
        let mut segs = t800_segments(2);
        segs[1].files.retain(|f| f.label != "Rear");
        let mut a = args(segs);
        a.duration_s = 100.0;
        let p = prepare(&a).unwrap();
        assert_eq!(p.labels, vec!["Front", "Interior"]);
        assert!(p.warnings.iter().any(|w| w.starts_with("Rear skipped")));
    }

    #[test]
    fn missing_main_channel_is_an_error() {
        let mut a = args(t800_segments(1));
        a.main_label = "Nope".into();
        assert!(prepare(&a).is_err());
    }

    #[test]
    fn main_can_be_any_channel_and_slots_can_be_chosen() {
        let mut a = args(t800_segments(1));
        a.main_label = "Rear".into();
        a.slot_labels = Some(vec!["Interior".into()]);
        a.audio = vec![];
        let p = prepare(&a).unwrap();
        assert_eq!(p.labels, vec!["Rear", "Interior"]);
        assert!(p.audio.is_empty());
        // GPS still comes from the master (Front), not from the chosen main.
        assert_eq!(p.gps_sources[0].path, "/f/0F.MP4");
    }

    #[test]
    fn audio_from_a_channel_that_is_not_shown_adds_an_input() {
        let mut a = args(t800_segments(1));
        a.slot_labels = Some(vec!["Rear".into()]);
        a.audio = vec![ExportAudio {
            label: "Interior".into(),
            volume: 0.8,
        }];
        let p = prepare(&a).unwrap();
        assert_eq!(p.labels, vec!["Front", "Rear", "Interior"]);
        assert_eq!(p.slots, vec![1]);
        assert_eq!(p.audio, vec![(2, 0.8)]);
        assert_eq!(p.list_texts.len(), 3);
    }

    #[test]
    fn two_audio_sources_and_bad_volume() {
        let mut a = args(t800_segments(1));
        a.audio = vec![
            ExportAudio {
                label: "Front".into(),
                volume: 1.0,
            },
            ExportAudio {
                label: "Interior".into(),
                volume: 0.5,
            },
        ];
        let p = prepare(&a).unwrap();
        assert_eq!(p.audio, vec![(0, 1.0), (1, 0.5)]);
        a.audio[1].volume = 9.0;
        assert!(prepare(&a).is_err());
        a.audio[1].volume = -1.0;
        assert!(prepare(&a).is_err());
    }

    #[test]
    fn unknown_slot_label_is_warned_not_fatal() {
        let mut a = args(t800_segments(1));
        a.slot_labels = Some(vec!["Roof".into(), "Rear".into()]);
        let p = prepare(&a).unwrap();
        assert_eq!(p.labels, vec!["Front", "Rear"]);
        assert!(p.warnings.iter().any(|w| w.starts_with("Roof skipped")));
    }
}
