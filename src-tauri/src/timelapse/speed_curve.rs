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

/// Render the curve as an ffmpeg `-filter_complex` body. Thin wrapper
/// that calls `build_curve` then formats each segment into a
/// `trim/setpts/scale` chain concatenated with the `concat` filter.
///
/// `scale_filter` is either `"scale"` (CPU/libx265) or `"scale_cuda"`
/// (NVENC). The rest of the filter graph is metadata-only (`trim`,
/// `setpts`, `concat`) and works identically on CPU or cuda frames.
pub fn compose_filter(
    windows: &[EventWindow],
    tier: Tier,
    total_duration_s: f64,
    scale_filter: &str,
) -> String {
    let curve = build_curve(windows, tier, total_duration_s);

    // Single-segment (fixed tier or no windows): render as a simple
    // passthrough with the same `[0:v]...[out]` shape as the concat
    // variant. This matches the pre-refactor output exactly.
    if curve.len() == 1 {
        return format!(
            "[0:v]setpts=PTS/{},{scale_filter}={OUT_WIDTH}:-2[out]",
            curve[0].rate
        );
    }

    let n = curve.len();
    let mut body = String::new();
    for (i, seg) in curve.iter().enumerate() {
        let pad = format!("s{i}");
        body.push_str(&format!(
            "[0:v]trim={:.3}:{:.3},setpts=PTS-STARTPTS,setpts=PTS/{},{scale_filter}={OUT_WIDTH}:-2[{pad}];",
            seg.concat_start, seg.concat_end, seg.rate
        ));
    }
    for i in 0..n {
        body.push_str(&format!("[s{i}]"));
    }
    body.push_str(&format!("concat=n={n}:v=1[out]"));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: f64, end: f64) -> EventWindow {
        EventWindow { start_s: start, end_s: end }
    }

    // Most tests exercise the CPU scale variant since the existing
    // assertions are written against it; a dedicated test covers the
    // NVENC / scale_cuda substitution.
    const CPU: &str = "scale";

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

    // ── compose_filter (regression guard: byte-identical to pre-refactor) ─

    #[test]
    fn fixed_tier_is_single_pass() {
        let got = compose_filter(&[], Tier::Tier8x, 300.0, CPU);
        assert_eq!(got, "[0:v]setpts=PTS/8,scale=1920:-2[out]");
    }

    #[test]
    fn fixed_tier_ignores_windows() {
        // 8x has base == event, so even with windows we should get
        // the single-pass form.
        let got = compose_filter(&[w(10.0, 20.0)], Tier::Tier8x, 300.0, CPU);
        assert_eq!(got, "[0:v]setpts=PTS/8,scale=1920:-2[out]");
    }

    #[test]
    fn variable_tier_with_no_windows_is_single_pass() {
        let got = compose_filter(&[], Tier::Tier16x, 300.0, CPU);
        assert_eq!(got, "[0:v]setpts=PTS/16,scale=1920:-2[out]");
    }

    #[test]
    fn variable_tier_with_one_middle_window_has_three_segments() {
        let got = compose_filter(&[w(60.0, 80.0)], Tier::Tier16x, 300.0, CPU);
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
        assert!(got.contains("PTS/16"), "base rate PTS/16 missing: {got}");
        assert!(got.contains("PTS/1,"), "event rate PTS/1 missing: {got}");
        assert!(got.contains("concat=n=3:v=1[out]"));
    }

    #[test]
    fn variable_tier_window_at_start_skips_leading() {
        let got = compose_filter(&[w(0.0, 10.0)], Tier::Tier60x, 120.0, CPU);
        // Two parts: [0-10 @ 8x], [10-120 @ 60x]
        assert!(got.contains("concat=n=2:v=1[out]"));
        assert!(got.contains("trim=0.000:10.000"));
        assert!(got.contains("trim=10.000:120.000"));
        assert!(got.contains("PTS/8,"));
        assert!(got.contains("PTS/60"));
    }

    #[test]
    fn variable_tier_window_at_end_skips_trailing() {
        let got = compose_filter(&[w(100.0, 120.0)], Tier::Tier16x, 120.0, CPU);
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
        );
        // Five parts: base, event, base, event, base
        assert!(got.contains("concat=n=5:v=1[out]"));
    }

    #[test]
    fn variable_tier_clamps_window_end_at_duration() {
        let got = compose_filter(&[w(80.0, 200.0)], Tier::Tier16x, 100.0, CPU);
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
        let got = compose_filter(&[w(50.0, 50.0)], Tier::Tier16x, 100.0, CPU);
        // Degenerate window — filter should reduce to single-pass.
        assert_eq!(got, "[0:v]setpts=PTS/16,scale=1920:-2[out]");
    }

    #[test]
    fn variable_tier_window_covering_whole_trip() {
        let got = compose_filter(&[w(0.0, 100.0)], Tier::Tier16x, 100.0, CPU);
        // Single-segment curve (event covers the whole trip) collapses
        // to the simple passthrough form — no concat filter needed.
        // Functionally equivalent to the pre-refactor 1-way concat.
        assert_eq!(got, "[0:v]setpts=PTS/1,scale=1920:-2[out]");
    }

    #[test]
    fn filter_body_is_identical_across_invocations() {
        // Sanity: the function is pure in its inputs. Two calls with
        // identical args must produce byte-identical strings. This is
        // what guarantees front/interior/rear stay synced when we run
        // the same filter against each channel.
        let a = compose_filter(&[w(10.0, 20.0)], Tier::Tier16x, 60.0, CPU);
        let b = compose_filter(&[w(10.0, 20.0)], Tier::Tier16x, 60.0, CPU);
        assert_eq!(a, b);
    }

    #[test]
    fn zero_duration_is_tolerated() {
        let got = compose_filter(&[w(0.0, 10.0)], Tier::Tier16x, 0.0, CPU);
        // Falls back to single-pass base rate.
        assert_eq!(got, "[0:v]setpts=PTS/16,scale=1920:-2[out]");
    }

    #[test]
    fn cuda_scale_filter_is_substituted_throughout() {
        // Fixed tier: scale_cuda appears once.
        let got = compose_filter(&[], Tier::Tier8x, 300.0, "scale_cuda");
        assert!(got.contains("scale_cuda=1920:-2"), "fixed tier: {got}");
        assert!(!got.contains("scale=1920"), "CPU scale leaked: {got}");

        // Variable tier: scale_cuda appears in every sub-stage.
        let got = compose_filter(
            &[w(10.0, 20.0), w(50.0, 60.0)],
            Tier::Tier16x,
            100.0,
            "scale_cuda",
        );
        let cuda_count = got.matches("scale_cuda=1920:-2").count();
        assert_eq!(cuda_count, 5, "expected one scale_cuda per stage: {got}");
        assert!(!got.contains("scale=1920"));
    }
}
