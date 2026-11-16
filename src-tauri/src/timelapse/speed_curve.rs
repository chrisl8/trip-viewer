//! Compose the ffmpeg `filter_complex` string that implements a
//! tier's variable-speed curve, and expose the underlying piecewise
//! curve as structured data so the frontend player can map between
//! file-time (what the `<video>` reports) and concat-time (trip-time).
//!
//! Every (trip, tier) pair produces exactly one filter string. The
//! caller then runs the same filter verbatim for front / interior /
//! rear so the three channels stay frame-perfectly synced — the GPS
//! windows are computed once at the trip level and don't depend on
//! which channel we're encoding.
//!
//! Output shape, always:
//!   `[0:v]...[out]`
//! so the ffmpeg invocation is uniformly
//!   `-filter_complex "<body>" -map "[out]"`
//! for both fixed and variable tiers. Keeps the encoder args simple.

use serde::{Deserialize, Serialize};

use crate::timelapse::types::{EventWindow, Tier};

/// Target output width. Height keeps aspect via `-2` (even number).
/// 1080p is plenty for a fast-scrubbing review — original 4K is
/// wasted pixels at 8x+ playback.
const OUT_WIDTH: u32 = 1920;

/// One piece of the speed curve: over the trip-time (concat-time)
/// range `[concat_start, concat_end]` the output plays at `rate`
/// concat-seconds per file-second. A rate of 8 means 8 s of trip
/// time is compressed into 1 s of the output MP4.
///
/// Serialized in camelCase to match the frontend's TypeScript type
/// and persisted on `timelapse_jobs.speed_curve_json`. Self-describing
/// so playback stays stable across tier-rate tweaks in the code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurveSegment {
    pub concat_start: f64,
    pub concat_end: f64,
    pub rate: u32,
}

/// Current persisted-curve schema version. Bump when the segment shape
/// or curve semantics change in a way readers can't transparently
/// absorb. Old rows lacking a `version` field (the bare-array form,
/// pre-versioning) are accepted by `deserialize_curve` and treated as
/// v1 — that's the only legacy shape in the wild.
pub const CURRENT_CURVE_VERSION: u32 = 1;

/// On-disk envelope around the segment list. Adds a `version` tag so a
/// future segment-shape change can be detected and either migrated or
/// rejected by a reader that doesn't recognize the version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurveEnvelope {
    version: u32,
    segments: Vec<CurveSegment>,
}

/// Serialize a curve to its persisted JSON form (versioned envelope).
/// Use this everywhere we write `timelapse_jobs.speed_curve_json` so
/// every fresh row carries a version tag.
pub fn serialize_curve(segments: &[CurveSegment]) -> String {
    let envelope = CurveEnvelope {
        version: CURRENT_CURVE_VERSION,
        segments: segments.to_vec(),
    };
    // The envelope shape is fixed — serialization can only fail under
    // exotic conditions (a number that doesn't round-trip), and the
    // empty-array fallback keeps callers from having to handle errors
    // for what is effectively infallible.
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        format!("{{\"version\":{CURRENT_CURVE_VERSION},\"segments\":[]}}")
    })
}

/// Parse a persisted curve JSON. Accepts both:
///  - Versioned envelope: `{"version": N, "segments": [...]}` (current
///    writers always emit this).
///  - Bare array: `[...]` (legacy pre-versioning rows already on disk).
///
/// Unknown future versions return None — readers should fall back to a
/// linear curve at the tier's base rate, the same as for any other
/// parse failure.
pub fn deserialize_curve(json: &str) -> Option<Vec<CurveSegment>> {
    if json.trim_start().starts_with('[') {
        return serde_json::from_str::<Vec<CurveSegment>>(json).ok();
    }
    let envelope: CurveEnvelope = serde_json::from_str(json).ok()?;
    if envelope.version != CURRENT_CURVE_VERSION {
        return None;
    }
    Some(envelope.segments)
}

/// Build the structured speed curve. This is the single source of
/// truth: `compose_filter` renders it to an ffmpeg filter string, and
/// the worker serializes it to JSON for the frontend player to use in
/// its file-time ↔ concat-time mapper.
///
/// Clips windows to `[0, total_duration_s]` and drops zero-width ones
/// defensively. Returns exactly one segment for fixed tiers (or when
/// variable-tier windows produce nothing usable after clipping).
pub fn build_curve(
    windows: &[EventWindow],
    tier: Tier,
    total_duration_s: f64,
) -> Vec<CurveSegment> {
    if total_duration_s <= 0.0 {
        // Degenerate — the worker shouldn't call us like this, but we
        // still produce a well-formed 1-element curve so downstream
        // code (and the filter renderer) has nothing to special-case.
        return vec![CurveSegment {
            concat_start: 0.0,
            concat_end: 0.0,
            rate: tier.base_rate(),
        }];
    }

    let base_rate = tier.base_rate();
    let event_rate = tier.event_rate();

    // Fixed tier, or variable tier with no usable windows: single span.
    let clipped: Vec<EventWindow> = if tier.is_variable() {
        windows
            .iter()
            .filter_map(|w| {
                let start = w.start_s.max(0.0);
                let end = w.end_s.min(total_duration_s);
                if end <= start {
                    None
                } else {
                    Some(EventWindow { start_s: start, end_s: end })
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    if clipped.is_empty() {
        return vec![CurveSegment {
            concat_start: 0.0,
            concat_end: total_duration_s,
            rate: base_rate,
        }];
    }

    // Alternating base / event / base / ... segments.
    let mut out: Vec<CurveSegment> = Vec::with_capacity(clipped.len() * 2 + 1);
    let mut cursor = 0.0;
    for w in &clipped {
        if w.start_s > cursor {
            out.push(CurveSegment {
                concat_start: cursor,
                concat_end: w.start_s,
                rate: base_rate,
            });
        }
        out.push(CurveSegment {
            concat_start: w.start_s,
            concat_end: w.end_s,
            rate: event_rate,
        });
        cursor = w.end_s;
    }
    if cursor < total_duration_s {
        out.push(CurveSegment {
            concat_start: cursor,
            concat_end: total_duration_s,
            rate: base_rate,
        });
    }
    out
}

/// Restrict a trip-level curve to the concat-time ranges a single
/// channel actually has footage for, producing that channel's
/// (possibly gappy) curve.
///
/// When a camera is off for part of a trip, its timelapse is built from
/// real footage only — no black filler — so its curve must omit the
/// missing ranges. Each `covered` range is intersected with the trip
/// curve segments, carrying each segment's rate; ranges with no footage
/// simply produce no segment, and the player reads the resulting
/// non-contiguity as a gap (hold + black overlay) at playback time.
///
/// `covered` must be sorted and disjoint; pass *maximal* runs of present
/// segments (merge adjacent present siblings) so the output curve stays
/// minimal. A single full-trip range reproduces `trip_curve` exactly.
pub fn restrict_curve_to_coverage(
    trip_curve: &[CurveSegment],
    covered: &[(f64, f64)],
) -> Vec<CurveSegment> {
    let mut out: Vec<CurveSegment> = Vec::new();
    for &(cs, ce) in covered {
        if ce <= cs {
            continue;
        }
        for seg in trip_curve {
            let start = seg.concat_start.max(cs);
            let end = seg.concat_end.min(ce);
            if end > start {
                out.push(CurveSegment {
                    concat_start: start,
                    concat_end: end,
                    rate: seg.rate,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.concat_start
            .partial_cmp(&b.concat_start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Minimum output length (seconds) for any single curve segment. A
/// per-window NVENC encode whose output is shorter than this fails to
/// open ("Error while opening encoder — incorrect parameters such as
/// bit_rate, rate, width or height"). Empirically ~0.05 s outputs encode
/// fine and ~0.025 s fail, so 0.1 s leaves a comfortable margin while
/// staying far too short to perceive any pacing change.
pub const MIN_WINDOW_OUTPUT_S: f64 = 0.1;

/// Guarantee every segment produces at least `min_output_s` of output
/// video, so the per-window encoder never chokes on a degenerately short
/// window. A segment's output length is `span / rate`; coverage-boundary
/// clipping and closely-spaced event windows can drop that below the
/// floor (this is what made the gappy 16x/60x rear encodes fail, and is a
/// latent hazard for any trip with near-adjacent events).
///
/// Two cases, neither of which changes a segment's *span* — so the curve
/// still tiles its coverage exactly and stays aligned with the gap-closed
/// source the encoder reads:
///  - A sliver whose *span* is already below `min_output_s` can't reach
///    the floor even at rate 1, so it's absorbed into the previous
///    contiguous segment (kept at that segment's rate). Across a gap there
///    is no previous neighbour, so an isolated sub-floor run is left at
///    rate 1 — best effort; covered runs are whole clips and in practice
///    always longer than this.
///  - Otherwise the rate is lowered just enough that `span / rate >=
///    min_output_s`. The affected stretch is always shorter than
///    `min_output_s` of output, so the slight slowdown is invisible.
pub fn sanitize_for_encode(curve: &[CurveSegment], min_output_s: f64) -> Vec<CurveSegment> {
    let mut out: Vec<CurveSegment> = Vec::with_capacity(curve.len());
    for seg in curve {
        let span = seg.concat_end - seg.concat_start;
        if span <= 0.0 {
            continue;
        }
        // Absorb a too-short-span sliver into the previous contiguous segment.
        if span < min_output_s {
            if let Some(prev) = out.last_mut() {
                if (seg.concat_start - prev.concat_end).abs() < 1e-6 {
                    prev.concat_end = seg.concat_end;
                    continue;
                }
            }
        }
        // Lower the rate just enough to reach the minimum output length.
        let mut rate = seg.rate;
        if (span / rate as f64) < min_output_s {
            rate = ((span / min_output_s).floor() as u32).clamp(1, seg.rate);
        }
        out.push(CurveSegment {
            concat_start: seg.concat_start,
            concat_end: seg.concat_end,
            rate,
        });
    }
    out
}

/// Collapse the gaps out of a (possibly gappy) channel curve, yielding
/// a contiguous curve in *source-time* — the timeline of the channel's
/// gap-closed real-footage source that ffmpeg actually encodes from.
///
/// The persisted curve is in concat-time (with gaps where the camera was
/// off) for the player; the encoder needs the same segments laid back-to-
/// back from 0 so its per-window `-ss` seeks land in the real source. The
/// per-segment spans and rates are identical — only the concat positions
/// shift. A contiguous (full-coverage) curve is returned unchanged.
pub fn collapse_gaps(curve: &[CurveSegment]) -> Vec<CurveSegment> {
    let mut out = Vec::with_capacity(curve.len());
    let mut cursor = 0.0;
    for seg in curve {
        let span = seg.concat_end - seg.concat_start;
        out.push(CurveSegment {
            concat_start: cursor,
            concat_end: cursor + span,
            rate: seg.rate,
        });
        cursor += span;
    }
    out
}

/// Render the curve as an ffmpeg `-filter_complex` body. Thin wrapper
/// that calls `build_curve` then formats each segment into a
/// `trim/setpts` chain concatenated with the `concat` filter.
///
/// `scale_filter` is either `"scale"` (CPU/libx265) or `"scale_cuda"`
/// (NVENC). `input_label` is the ffmpeg pad name the head reads from
/// — typically `"0:v"` for a single source, or a label like `"vcat"`
/// when the caller has already prepended a concat filter that joins
/// multiple inputs into one stream.
///
/// Production code now builds the curve once in the worker and goes
/// straight to `compose_filter_from_curve`; this wrapper is kept for
/// the test suite (which exercises the curve→filter mapping by tier
/// and window shape rather than by curve directly) and as the public
/// entry-point should an external caller want it.
#[allow(dead_code)]
pub fn compose_filter(
    windows: &[EventWindow],
    tier: Tier,
    total_duration_s: f64,
    scale_filter: &str,
    input_label: &str,
) -> String {
    let curve = build_curve(windows, tier, total_duration_s);
    compose_filter_from_curve(&curve, scale_filter, input_label)
}

/// Same as `compose_filter` but takes a pre-built curve. Used by the
/// encode dispatcher in `ffmpeg.rs`, which builds the curve once per
/// job and uses it for both filter composition (single-shot path) and
/// the JSON metadata persisted to `timelapse_jobs.speed_curve_json`.
/// Avoids the duplicate `build_curve` call.
pub fn compose_filter_from_curve(
    curve: &[CurveSegment],
    scale_filter: &str,
    input_label: &str,
) -> String {
    // Head normalization stage. CPU `scale` and CUDA `scale_cuda`
    // differ in how pix_fmt is set: scale needs a separate `format=`
    // filter (host frames), scale_cuda accepts pix_fmt as an option
    // (CUDA frames stay on the GPU).
    let head = if scale_filter == "scale_cuda" {
        format!("scale_cuda={OUT_WIDTH}:-2:format=yuv420p")
    } else if scale_filter == "scale_vaapi" {
        format!("format=nv12,hwupload,scale_vaapi={OUT_WIDTH}:-2")
    } else {
        format!("format=yuv420p,{scale_filter}={OUT_WIDTH}:-2")
    };

    // Single-segment (fixed tier or no windows): no trim needed —
    // just normalize and apply the global rate change.
    if curve.len() <= 1 {
        let rate = curve.first().map(|s| s.rate).unwrap_or(1);
        return format!("[{input_label}]{head},setpts=PTS/{rate}[out]");
    }

    let n = curve.len();
    let mut body = String::new();
    // Normalize once, then split into N labeled outputs that each
    // per-curve-segment trim/setpts chain consumes. The split filter
    // is what lets us reuse a single normalized stream across N trims.
    body.push_str(&format!("[{input_label}]{head},split={n}"));
    for i in 0..n {
        body.push_str(&format!("[v{i}]"));
    }
    body.push(';');
    for (i, seg) in curve.iter().enumerate() {
        body.push_str(&format!(
            "[v{i}]trim={:.3}:{:.3},setpts=PTS-STARTPTS,setpts=PTS/{}[s{i}];",
            seg.concat_start, seg.concat_end, seg.rate
        ));
    }
    for i in 0..n {
        body.push_str(&format!("[s{i}]"));
    }
    body.push_str(&format!("concat=n={n}:v=1[out]"));
    body
}

/// Build the per-window filter for the multi-window encoding path.
/// Single input, scale-and-format normalization, then `setpts=PTS/rate`.
/// No split, no concat — this is the whole point of the multi-window
/// path: one stream end-to-end per ffmpeg, so memory stays bounded by
/// the decoder's own queues regardless of how many windows the curve
/// has.
pub fn compose_window_filter(scale_filter: &str, rate: u32) -> String {
    if scale_filter == "scale_cuda" {
        format!("[0:v]scale_cuda={OUT_WIDTH}:-2:format=yuv420p,setpts=PTS/{rate}[out]")
    } else if scale_filter == "scale_vaapi" {
        format!("[0:v]format=nv12,hwupload,scale_vaapi={OUT_WIDTH}:-2,setpts=PTS/{rate}[out]")
    } else {
        format!("[0:v]format=yuv420p,{scale_filter}={OUT_WIDTH}:-2,setpts=PTS/{rate}[out]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vaapi_upload_precedes_gpu_scaling_and_time_mapping() {
        let filter = compose_window_filter("scale_vaapi", 8);
        assert!(filter.starts_with("[0:v]format=nv12,hwupload,scale_vaapi="));
        assert!(filter.ends_with(",setpts=PTS/8[out]"));
        assert!(!filter.contains("cuda"));
        let curve = build_curve(&[], Tier::Tier8x, 16.0);
        assert_eq!(compose_filter_from_curve(&curve, "scale_vaapi", "0:v"), filter);
    }

    fn w(start: f64, end: f64) -> EventWindow {
        EventWindow { start_s: start, end_s: end }
    }

    // Most tests exercise the CPU scale variant since the existing
    // assertions are written against it; a dedicated test covers the
    // NVENC / scale_cuda substitution.
    const CPU: &str = "scale";
    /// Default input label for tests that pre-date the input-label
    /// param. Keeps legacy assertions on `[0:v]` head shape stable.
    const LBL: &str = "0:v";

    // ── envelope round-trip ──────────────────────────────────────────

    #[test]
    fn serialize_curve_emits_versioned_envelope() {
        let curve = vec![CurveSegment {
            concat_start: 0.0,
            concat_end: 60.0,
            rate: 8,
        }];
        let json = serialize_curve(&curve);
        assert!(
            json.contains("\"version\":1"),
            "expected version tag in output: {json}"
        );
        assert!(
            json.contains("\"segments\""),
            "expected segments key in output: {json}"
        );
    }

    #[test]
    fn deserialize_curve_round_trips_via_envelope() {
        let curve = vec![
            CurveSegment { concat_start: 0.0, concat_end: 25.0, rate: 16 },
            CurveSegment { concat_start: 25.0, concat_end: 40.0, rate: 1 },
            CurveSegment { concat_start: 40.0, concat_end: 60.0, rate: 16 },
        ];
        let json = serialize_curve(&curve);
        let back = deserialize_curve(&json).expect("envelope should parse");
        assert_eq!(back, curve);
    }

    #[test]
    fn deserialize_curve_accepts_legacy_bare_array() {
        // Pre-versioning shape — what existing rows in the wild look
        // like. Must continue to play back without re-encoding.
        let legacy = "[{\"concatStart\":0.0,\"concatEnd\":300.0,\"rate\":8}]";
        let parsed = deserialize_curve(legacy).expect("legacy should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].rate, 8);
        assert_eq!(parsed[0].concat_end, 300.0);
    }

    #[test]
    fn deserialize_curve_rejects_unknown_future_version() {
        let json = "{\"version\":99,\"segments\":[]}";
        assert!(
            deserialize_curve(json).is_none(),
            "unknown version should be rejected so caller falls back"
        );
    }

    #[test]
    fn deserialize_curve_rejects_garbage() {
        assert!(deserialize_curve("not json").is_none());
        assert!(deserialize_curve("").is_none());
        assert!(deserialize_curve("{\"version\":1}").is_none()); // missing segments
    }

    // ── build_curve ───────────────────────────────────────────────────

    #[test]
    fn build_curve_fixed_tier_is_one_segment() {
        let c = build_curve(&[], Tier::Tier8x, 300.0);
        assert_eq!(
            c,
            vec![CurveSegment {
                concat_start: 0.0,
                concat_end: 300.0,
                rate: 8
            }]
        );
    }

    #[test]
    fn build_curve_variable_tier_without_windows_is_one_segment() {
        let c = build_curve(&[], Tier::Tier16x, 300.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].rate, 16);
    }

    #[test]
    fn build_curve_variable_tier_with_middle_window_has_three_segments() {
        // Plan example: 60 s trip, 16x tier, event at [25, 40].
        let c = build_curve(&[w(25.0, 40.0)], Tier::Tier16x, 60.0);
        assert_eq!(
            c,
            vec![
                CurveSegment { concat_start: 0.0, concat_end: 25.0, rate: 16 },
                CurveSegment { concat_start: 25.0, concat_end: 40.0, rate: 1 },
                CurveSegment { concat_start: 40.0, concat_end: 60.0, rate: 16 },
            ]
        );
    }

    #[test]
    fn build_curve_variable_tier_clamps_overshoot_windows() {
        let c = build_curve(&[w(-5.0, 10.0), w(90.0, 200.0)], Tier::Tier16x, 100.0);
        // Two event segments plus the 10-90 gap at base rate.
        assert_eq!(
            c,
            vec![
                CurveSegment { concat_start: 0.0, concat_end: 10.0, rate: 1 },
                CurveSegment { concat_start: 10.0, concat_end: 90.0, rate: 16 },
                CurveSegment { concat_start: 90.0, concat_end: 100.0, rate: 1 },
            ]
        );
    }

    #[test]
    fn build_curve_zero_duration_returns_well_formed_curve() {
        let c = build_curve(&[w(0.0, 5.0)], Tier::Tier16x, 0.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].concat_start, 0.0);
        assert_eq!(c[0].concat_end, 0.0);
    }

    #[test]
    fn build_curve_is_serde_roundtrippable() {
        let curve = build_curve(&[w(25.0, 40.0)], Tier::Tier16x, 60.0);
        let json = serde_json::to_string(&curve).unwrap();
        // Matches the camelCase contract the frontend expects.
        assert!(json.contains("concatStart"));
        assert!(json.contains("concatEnd"));
        let parsed: Vec<CurveSegment> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, curve);
    }

    // ── restrict_curve_to_coverage ────────────────────────────────────

    fn seg(s: f64, e: f64, r: u32) -> CurveSegment {
        CurveSegment { concat_start: s, concat_end: e, rate: r }
    }

    // Trip curve used across the coverage tests: 16x with a 1x event at
    // [60,75], like the player examples.
    fn trip() -> Vec<CurveSegment> {
        vec![seg(0.0, 60.0, 16), seg(60.0, 75.0, 1), seg(75.0, 300.0, 16)]
    }

    #[test]
    fn restrict_full_coverage_reproduces_trip_curve() {
        let got = restrict_curve_to_coverage(&trip(), &[(0.0, 300.0)]);
        assert_eq!(got, trip());
    }

    #[test]
    fn restrict_middle_gap_drops_event_inside_gap() {
        // Camera off over [30,90] — the [60,75] event is entirely inside
        // the gap, so it must not appear in the channel's curve.
        let got = restrict_curve_to_coverage(&trip(), &[(0.0, 30.0), (90.0, 300.0)]);
        assert_eq!(got, vec![seg(0.0, 30.0, 16), seg(90.0, 300.0, 16)]);
    }

    #[test]
    fn restrict_leading_gap() {
        let got = restrict_curve_to_coverage(&trip(), &[(90.0, 300.0)]);
        assert_eq!(got, vec![seg(90.0, 300.0, 16)]);
    }

    #[test]
    fn restrict_keeps_partial_event_overlap() {
        // Coverage ends mid-event: keep the base run and the clipped event.
        let got = restrict_curve_to_coverage(&trip(), &[(0.0, 70.0)]);
        assert_eq!(got, vec![seg(0.0, 60.0, 16), seg(60.0, 70.0, 1)]);
    }

    #[test]
    fn sanitize_clamps_rate_of_short_high_speed_fragment() {
        // The May 18 rear failure: an isolated 0.4s covered run at 16x →
        // 0.025s output → NVENC won't open. Clamp to 0.4/0.1 = 4x.
        let got = sanitize_for_encode(&[seg(100.0, 100.4, 16)], 0.1);
        assert_eq!(got, vec![seg(100.0, 100.4, 4)]);
        // At 60x the same fragment also clamps to 4x.
        let got60 = sanitize_for_encode(&[seg(100.0, 100.4, 60)], 0.1);
        assert_eq!(got60, vec![seg(100.0, 100.4, 4)]);
    }

    #[test]
    fn sanitize_leaves_long_segments_untouched() {
        // 6s @ 60x = exactly 0.1s output → at the floor, not below → kept.
        assert_eq!(
            sanitize_for_encode(&[seg(0.0, 6.0, 60)], 0.1),
            vec![seg(0.0, 6.0, 60)]
        );
        // A normal long base run is well above the floor.
        assert_eq!(
            sanitize_for_encode(&[seg(0.0, 300.0, 16)], 0.1),
            vec![seg(0.0, 300.0, 16)]
        );
    }

    #[test]
    fn sanitize_absorbs_subfloor_sliver_into_previous_contiguous_segment() {
        // A 0.02s base sliver between two segments (e.g. event-boundary
        // clipping) is absorbed into the previous segment, not encoded.
        let got = sanitize_for_encode(
            &[seg(0.0, 60.0, 16), seg(60.0, 60.02, 16), seg(60.02, 75.0, 1)],
            0.1,
        );
        assert_eq!(got, vec![seg(0.0, 60.02, 16), seg(60.02, 75.0, 1)]);
    }

    #[test]
    fn sanitize_does_not_merge_across_a_gap() {
        // A sub-floor isolated run after a gap has no contiguous prev, so
        // it's left at rate 1 (best effort) rather than merged across the gap.
        let got = sanitize_for_encode(&[seg(0.0, 30.0, 16), seg(90.0, 90.05, 16)], 0.1);
        assert_eq!(got, vec![seg(0.0, 30.0, 16), seg(90.0, 90.05, 1)]);
    }

    #[test]
    fn collapse_gaps_makes_curve_contiguous_preserving_spans_and_rates() {
        let gappy = vec![seg(0.0, 30.0, 16), seg(90.0, 300.0, 16)];
        let got = collapse_gaps(&gappy);
        // [0,30] stays; [90,300] (span 210) shifts to [30,240].
        assert_eq!(got, vec![seg(0.0, 30.0, 16), seg(30.0, 240.0, 16)]);
    }

    #[test]
    fn collapse_gaps_is_noop_for_contiguous_curve() {
        assert_eq!(collapse_gaps(&trip()), trip());
    }

    #[test]
    fn restrict_then_collapse_full_coverage_is_identity() {
        // Full-coverage channels must encode and persist exactly as today.
        let persist = restrict_curve_to_coverage(&trip(), &[(0.0, 300.0)]);
        let source = collapse_gaps(&persist);
        assert_eq!(persist, trip());
        assert_eq!(source, trip());
    }

    #[test]
    fn restrict_empty_coverage_is_empty() {
        assert!(restrict_curve_to_coverage(&trip(), &[]).is_empty());
        // Degenerate zero-width ranges contribute nothing.
        assert!(restrict_curve_to_coverage(&trip(), &[(50.0, 50.0)]).is_empty());
    }

    #[test]
    fn restrict_output_is_sorted_and_within_coverage() {
        let got = restrict_curve_to_coverage(&trip(), &[(0.0, 30.0), (90.0, 300.0)]);
        for w in got.windows(2) {
            assert!(w[0].concat_start <= w[1].concat_start);
        }
        // Nothing leaks into the [30,90] gap.
        assert!(got.iter().all(|s| s.concat_end <= 30.0 || s.concat_start >= 90.0));
    }

    // ── compose_filter ────────────────────────────────────────────────
    // Head shape is `[0:v]format=yuv420p,scale=1920:-2,...` for CPU
    // and `[0:v]scale_cuda=1920:-2:format=yuv420p,...` for CUDA. This
    // normalization is the load-bearing fix for the filter-reinit
    // error on heterogeneous source files (different resolution /
    // pix_fmt across segments, or real-file vs black-placeholder).

    #[test]
    fn fixed_tier_is_single_pass() {
        let got = compose_filter(&[], Tier::Tier8x, 300.0, CPU, LBL);
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/8[out]");
    }

    #[test]
    fn fixed_tier_ignores_windows() {
        // 8x has base == event, so even with windows we should get
        // the single-pass form.
        let got = compose_filter(&[w(10.0, 20.0)], Tier::Tier8x, 300.0, CPU, LBL);
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/8[out]");
    }

    #[test]
    fn variable_tier_with_no_windows_is_single_pass() {
        let got = compose_filter(&[], Tier::Tier16x, 300.0, CPU, LBL);
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/16[out]");
    }

    #[test]
    fn variable_tier_with_one_middle_window_has_three_segments() {
        let got = compose_filter(&[w(60.0, 80.0)], Tier::Tier16x, 300.0, CPU, LBL);
        // Three parts: [0-60 @ 16x], [60-80 @ 1x], [80-300 @ 16x]
        assert!(
            got.contains("trim=0.000:60.000"),
            "leading segment missing: {got}"
        );
        assert!(
            got.contains("trim=60.000:80.000"),
            "event segment missing: {got}"
        );
        assert!(
            got.contains("trim=80.000:300.000"),
            "trailing segment missing: {got}"
        );
        // Each per-segment chain now ends with `setpts=PTS/r[s{i}];`,
        // so look for the [s{i}] terminator to assert the rate.
        assert!(got.contains("PTS/16[s0]"), "base rate PTS/16 missing on s0: {got}");
        assert!(got.contains("PTS/1[s1]"), "event rate PTS/1 missing on s1: {got}");
        assert!(got.contains("PTS/16[s2]"), "base rate PTS/16 missing on s2: {got}");
        assert!(got.contains("concat=n=3:v=1[out]"));
    }

    #[test]
    fn variable_tier_window_at_start_skips_leading() {
        let got = compose_filter(&[w(0.0, 10.0)], Tier::Tier60x, 120.0, CPU, LBL);
        // Two parts: [0-10 @ 8x], [10-120 @ 60x]
        assert!(got.contains("concat=n=2:v=1[out]"));
        assert!(got.contains("trim=0.000:10.000"));
        assert!(got.contains("trim=10.000:120.000"));
        assert!(got.contains("PTS/8[s0]"));
        assert!(got.contains("PTS/60[s1]"));
    }

    #[test]
    fn variable_tier_window_at_end_skips_trailing() {
        let got = compose_filter(&[w(100.0, 120.0)], Tier::Tier16x, 120.0, CPU, LBL);
        assert!(got.contains("concat=n=2:v=1[out]"));
        assert!(got.contains("trim=0.000:100.000"));
        assert!(got.contains("trim=100.000:120.000"));
    }

    #[test]
    fn variable_tier_multiple_windows() {
        let got = compose_filter(
            &[w(10.0, 20.0), w(50.0, 60.0)],
            Tier::Tier16x,
            100.0,
            CPU,
            LBL,
        );
        // Five parts: base, event, base, event, base
        assert!(got.contains("concat=n=5:v=1[out]"));
    }

    #[test]
    fn variable_tier_clamps_window_end_at_duration() {
        let got = compose_filter(&[w(80.0, 200.0)], Tier::Tier16x, 100.0, CPU, LBL);
        // Window clamped to [80, 100]; trailing base segment should NOT exist.
        assert!(got.contains("concat=n=2:v=1[out]"));
        assert!(got.contains("trim=80.000:100.000"));
        assert!(
            !got.contains("trim=100.000"),
            "trailing segment should be absent: {got}"
        );
    }

    #[test]
    fn variable_tier_drops_zero_width_window() {
        let got = compose_filter(&[w(50.0, 50.0)], Tier::Tier16x, 100.0, CPU, LBL);
        // Degenerate window — filter should reduce to single-pass.
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/16[out]");
    }

    #[test]
    fn variable_tier_window_covering_whole_trip() {
        let got = compose_filter(&[w(0.0, 100.0)], Tier::Tier16x, 100.0, CPU, LBL);
        // Single-segment curve (event covers the whole trip) collapses
        // to the simple passthrough form — no concat filter needed.
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/1[out]");
    }

    #[test]
    fn filter_body_is_identical_across_invocations() {
        // Sanity: the function is pure in its inputs. Two calls with
        // identical args must produce byte-identical strings. This is
        // what guarantees front/interior/rear stay synced when we run
        // the same filter against each channel.
        let a = compose_filter(&[w(10.0, 20.0)], Tier::Tier16x, 60.0, CPU, LBL);
        let b = compose_filter(&[w(10.0, 20.0)], Tier::Tier16x, 60.0, CPU, LBL);
        assert_eq!(a, b);
    }

    #[test]
    fn zero_duration_is_tolerated() {
        let got = compose_filter(&[w(0.0, 10.0)], Tier::Tier16x, 0.0, CPU, LBL);
        // Falls back to single-pass base rate.
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/16[out]");
    }

    #[test]
    fn cuda_scale_filter_uses_inline_format_option() {
        // Fixed tier: scale_cuda appears once with format=yuv420p as
        // a parameter. No CPU `scale=` and no separate `format=`
        // filter — frames stay on the GPU end-to-end.
        let got = compose_filter(&[], Tier::Tier8x, 300.0, "scale_cuda", LBL);
        assert_eq!(
            got,
            "[0:v]scale_cuda=1920:-2:format=yuv420p,setpts=PTS/8[out]"
        );

        // Variable tier: still ONE scale_cuda at the head. The whole
        // point of the new filter shape is that resize + format
        // happens once before split, not per per-segment trim chain.
        let got = compose_filter(
            &[w(10.0, 20.0), w(50.0, 60.0)],
            Tier::Tier16x,
            100.0,
            "scale_cuda",
            LBL,
        );
        let cuda_count = got.matches("scale_cuda=1920:-2:format=yuv420p").count();
        assert_eq!(cuda_count, 1, "expected exactly one head scale_cuda: {got}");
        assert!(!got.contains(",scale="), "CPU scale leaked into CUDA path: {got}");
        // Sanity-check the structural shape:
        assert!(got.starts_with("[0:v]scale_cuda=1920:-2:format=yuv420p,split=5"));
        assert!(got.contains("concat=n=5:v=1[out]"));
    }

    #[test]
    fn input_label_is_substituted_into_head() {
        // Caller passes "vcat" (the output of an upstream concat
        // filter) — the head reads from that pad, not the default
        // [0:v]. Tests both fixed and variable curve shapes.
        let fixed = compose_filter(&[], Tier::Tier8x, 300.0, CPU, "vcat");
        assert_eq!(fixed, "[vcat]format=yuv420p,scale=1920:-2,setpts=PTS/8[out]");

        let var = compose_filter(&[w(60.0, 80.0)], Tier::Tier16x, 300.0, CPU, "vcat");
        assert!(
            var.starts_with("[vcat]format=yuv420p,scale=1920:-2,split=3[v0][v1][v2];"),
            "head shape wrong: {var}"
        );
    }

    #[test]
    fn window_filter_cpu_shape() {
        let got = compose_window_filter("scale", 16);
        assert_eq!(got, "[0:v]format=yuv420p,scale=1920:-2,setpts=PTS/16[out]");
    }

    #[test]
    fn window_filter_cuda_shape() {
        let got = compose_window_filter("scale_cuda", 8);
        assert_eq!(
            got,
            "[0:v]scale_cuda=1920:-2:format=yuv420p,setpts=PTS/8[out]"
        );
    }

    #[test]
    fn window_filter_has_no_split_or_concat() {
        // Memory regression test: the whole reason this filter exists
        // is to avoid the split=N → trim → concat=N graph that was
        // pinning 12–36 GB per ffmpeg on variable-tier jobs. Make sure
        // those structures don't sneak back in.
        let got = compose_window_filter("scale_cuda", 60);
        assert!(!got.contains("split="));
        assert!(!got.contains("concat="));
        assert!(!got.contains("trim="));
    }

    #[test]
    fn multi_segment_normalizes_then_splits() {
        let got = compose_filter(&[w(60.0, 80.0)], Tier::Tier16x, 300.0, CPU, LBL);
        // Three-segment curve → split=3 then three trim chains.
        assert!(
            got.starts_with("[0:v]format=yuv420p,scale=1920:-2,split=3[v0][v1][v2];"),
            "head shape wrong: {got}"
        );
        assert!(got.contains("[v0]trim=0.000:60.000,setpts=PTS-STARTPTS,setpts=PTS/16[s0];"));
        assert!(got.contains("[v1]trim=60.000:80.000,setpts=PTS-STARTPTS,setpts=PTS/1[s1];"));
        assert!(got.contains("[v2]trim=80.000:300.000,setpts=PTS-STARTPTS,setpts=PTS/16[s2];"));
        assert!(got.ends_with("[s0][s1][s2]concat=n=3:v=1[out]"));
        // Per-segment scale should NOT appear — normalization is
        // upstream of trim.
        assert_eq!(got.matches(",scale=").count(), 1, "scale should appear exactly once at head: {got}");
    }
}
