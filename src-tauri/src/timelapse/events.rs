//! GPS-driven detection of "interesting" moments in a trip that the
//! variable-speed tiers slow down for.
//!
//! Input is a trip's GPS trace, stitched across segments and remapped
//! to concat-timeline seconds (so timestamps line up with ffmpeg's
//! `trim` filter on the concat-demuxer output). Output is a list of
//! `EventWindow`s — start/end seconds where playback should switch to
//! the tier's slower `event_rate`.
//!
//! Detectors in v1 are tuned for the noise floor of a 1 Hz automotive
//! GPS trace, not a phone's IMU. Thresholds are conservative to avoid
//! surfacing every little speed wobble; we'd rather miss a borderline
//! event than turn a trip's timelapse into a strobe of rate changes.
//!
//! # Verifying and tuning
//!
//! After a 16x or 60x run completes, play one of the output files and
//! watch around a moment you *know* was eventful (a hard brake, a sharp
//! turn, a long stoplight). Expected behavior:
//!
//! - 16x timelapse: base 16x playback drops to **1x** for a ~15 s
//!   window centered on the event (5 s lead, 10 s trail — see
//!   `EVENT_LEAD_S` / `EVENT_TRAIL_S` below).
//! - 60x timelapse: same bracket, drops to **8x** instead of 1x.
//!
//! If the slowdowns feel **too jumpy** (many short transitions, every
//! little brake triggers), raise the threshold constants so only
//! stronger events register. If they feel **too lazy** (obvious events
//! go by at full base rate), lower them.
//!
//! Thresholds to adjust, in order of likely impact:
//! - `HARD_BRAKE_MPS2` / `HARD_ACCEL_MPS2` — the noisy ones during
//!   city driving. Typical dashcam noise floor sits around ±1.5 m/s².
//! - `SHARP_TURN_DEG_PER_S` — highway lane changes usually produce
//!   < 15 °/s; 30 °/s is roughly "evasive."
//! - `LONG_STOP_MIN_S` — drop to 60 s if you want traffic-light stops
//!   flagged; raise to 300 s if you only care about destination
//!   arrivals.
//! - `TRAFFIC_MIN_CROSSINGS` / `TRAFFIC_WINDOW_S` — how dense a
//!   stop-and-go run has to be before it reads as "traffic."
//! - `EVENT_LEAD_S` / `EVENT_TRAIL_S` — how much context to keep at
//!   normal speed around each event. Widen these before widening the
//!   detector thresholds if events feel clipped.
//!
//! After editing, rebuild (`cargo build --manifest-path src-tauri/Cargo.toml`)
//! and regenerate just the affected tiers: open Timelapse view, pick
//! 16x and/or 60x, set scope to **Rebuild all**, and Start. The fixed
//! 8x tier is unaffected by these constants and doesn't need to rerun.

use crate::model::GpsPoint;
use crate::timelapse::types::EventWindow;

/// Seconds to include before each detected event in the output window.
pub const EVENT_LEAD_S: f64 = 5.0;
/// Seconds to include after each detected event in the output window.
pub const EVENT_TRAIL_S: f64 = 10.0;

// ── Thresholds ────────────────────────────────────────────────────────
// Kept as module-level constants (not user-tunable in v1 — the plan
// calls for shipping defaults first and iterating). Roughly:
//   hard brake / accel: 0.3g and 0.35g
//   sharp turn:         30°/s sustained for at least two samples
//   long stop:          still for 2+ minutes
//   traffic cluster:    5+ stop-cycles in a minute

const HARD_BRAKE_MPS2: f64 = -4.0;
const HARD_ACCEL_MPS2: f64 = 4.0;
const SHARP_TURN_DEG_PER_S: f64 = 30.0;
const LONG_STOP_MPS: f64 = 1.0;
const LONG_STOP_MIN_S: f64 = 120.0;
const TRAFFIC_WINDOW_S: f64 = 60.0;
const TRAFFIC_MIN_CROSSINGS: usize = 5;

/// Minimum speed (m/s) for "moving" filters. Brake/accel/turn events
/// with either endpoint below this are treated as routine stop-start
/// behavior (traffic lights, pulling away from parking, intersection
/// turns) rather than interesting driving moments.
///
/// 2 m/s ≈ 4.5 mph. Fast enough to clearly be in motion; slow enough
/// that a legitimate mid-motion brake event still peaks above it.
const MOVING_MPS: f64 = 2.0;

/// Detect every event in the trip's GPS trace and return merged
/// windows (lead-in + event moment + trail-out, overlaps coalesced).
///
/// Points with `fix_ok == false` are dropped upstream of derivative
/// calculations so bogus position/speed during fix loss can't forge
/// events. Callers should not pre-filter — the function is written
/// against the raw trace so it can preserve the caller's intended
/// concat-timeline offsets on every point.
pub fn detect_events(gps: &[GpsPoint]) -> Vec<EventWindow> {
    let mut timestamps: Vec<f64> = Vec::new();

    timestamps.extend(detect_speed_spikes(gps));
    timestamps.extend(detect_sharp_turns(gps));
    timestamps.extend(detect_long_stops(gps));
    timestamps.extend(detect_traffic_clusters(gps));

    timestamps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut windows: Vec<EventWindow> = timestamps
        .into_iter()
        .map(|t| EventWindow {
            start_s: (t - EVENT_LEAD_S).max(0.0),
            end_s: t + EVENT_TRAIL_S,
        })
        .collect();

    merge_overlapping(&mut windows);
    windows
}

/// Hard brake (< HARD_BRAKE_MPS2) and hard accel (> HARD_ACCEL_MPS2)
/// moments. Emits one timestamp per crossing — the sample where the
/// derivative first exceeds threshold.
///
/// Filters applied to ignore routine traffic behavior:
/// - Brake: require speed *after* the brake ≥ MOVING_MPS, so
///   "brake-to-a-stop" at a light doesn't fire. A real panic stop
///   peaks at mid-brake while still in motion — still catches it.
/// - Accel: require speed *before* the accel ≥ MOVING_MPS, so
///   "pull away from a stoplight" doesn't fire. A mid-cruise accel
///   from 40 → 50 mph still triggers.
fn detect_speed_spikes(gps: &[GpsPoint]) -> Vec<f64> {
    let mut out = Vec::new();
    for pair in gps.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if !a.fix_ok || !b.fix_ok {
            continue;
        }
        let dt = b.t_offset_s - a.t_offset_s;
        if dt <= 0.0 || dt > 5.0 {
            // Samples too far apart to trust a derivative across; GPS
            // drop-out or segment boundary. Conservative: skip.
            continue;
        }
        let dv = (b.speed_mps - a.speed_mps) / dt;
        let hard_brake = dv <= HARD_BRAKE_MPS2 && b.speed_mps >= MOVING_MPS;
        let hard_accel = dv >= HARD_ACCEL_MPS2 && a.speed_mps >= MOVING_MPS;
        if hard_brake || hard_accel {
            out.push(b.t_offset_s);
        }
    }
    out
}

/// Sharp turn: heading rate > 30°/s sustained across at least two
/// consecutive inter-sample pairs. At 1 Hz sampling a single pair's
/// dt IS already a second, so "sustained ≥ 1s" in the plan really
/// wants "at least two consecutive high-rate pairs". We emit at the
/// second such pair's endpoint so the window brackets the turn.
///
/// Filter: both GPS points must be moving (≥ MOVING_MPS). GPS
/// heading is unreliable at zero speed — it drifts by tens of
/// degrees per second with no real vehicle motion, triggering this
/// detector spuriously during every traffic-light wait. Requiring
/// actual motion kills that noise entirely; legitimate sharp turns
/// happen while driving, by definition.
fn detect_sharp_turns(gps: &[GpsPoint]) -> Vec<f64> {
    let mut out = Vec::new();
    // Tracks whether the *previous* pair already exceeded threshold.
    // The second consecutive exceed triggers the emit.
    let mut prev_exceed = false;

    for pair in gps.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if !a.fix_ok || !b.fix_ok {
            prev_exceed = false;
            continue;
        }
        let dt = b.t_offset_s - a.t_offset_s;
        if dt <= 0.0 || dt > 5.0 {
            prev_exceed = false;
            continue;
        }
        if a.speed_mps < MOVING_MPS || b.speed_mps < MOVING_MPS {
            prev_exceed = false;
            continue;
        }
        let rate = angular_delta(a.heading_deg, b.heading_deg).abs() / dt;
        if rate >= SHARP_TURN_DEG_PER_S {
            if prev_exceed {
                out.push(b.t_offset_s);
                // Reset so a sustained three-sample turn emits once,
                // not twice. The "second pair" was the trigger; after
                // emitting, require a fresh first pair to re-arm.
                prev_exceed = false;
            } else {
                prev_exceed = true;
            }
        } else {
            prev_exceed = false;
        }
    }
    out
}

/// Long stop: a run of ≥120s where speed stays below 1 m/s. Emits one
/// timestamp at the run's midpoint so the window brackets the "stop"
/// rather than the point the driver finally moved again.
fn detect_long_stops(gps: &[GpsPoint]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut run_start: Option<f64> = None;

    for p in gps {
        if !p.fix_ok {
            // Fix loss while stopped shouldn't break the run; keep
            // counting. Fix loss while moving is uncommon but if the
            // prior state was "no run", stay in no run.
            continue;
        }
        if p.speed_mps < LONG_STOP_MPS {
            if run_start.is_none() {
                run_start = Some(p.t_offset_s);
            }
        } else if let Some(start) = run_start.take() {
            let end = p.t_offset_s;
            if end - start >= LONG_STOP_MIN_S {
                out.push((start + end) / 2.0);
            }
        }
    }
    // Trip ends while still stopped.
    if let (Some(start), Some(last)) = (run_start, gps.last()) {
        let end = last.t_offset_s;
        if end - start >= LONG_STOP_MIN_S {
            out.push((start + end) / 2.0);
        }
    }
    out
}

/// Traffic cluster: ≥5 zero-crossings of speed=0 within a sliding 60s
/// window. Counts every transition from <1 m/s to >1 m/s (one end of a
/// stop-and-go cycle — the other end is the paired > → <). We look
/// at ends-of-stops rather than pairs to keep the detector one-pass.
fn detect_traffic_clusters(gps: &[GpsPoint]) -> Vec<f64> {
    // Collect stop-end timestamps (where we just left a <1 m/s state).
    let mut stop_ends: Vec<f64> = Vec::new();
    let mut in_stop = false;
    for p in gps {
        if !p.fix_ok {
            continue;
        }
        if p.speed_mps < LONG_STOP_MPS {
            in_stop = true;
        } else if in_stop {
            stop_ends.push(p.t_offset_s);
            in_stop = false;
        }
    }

    // Sliding window over stop_ends: any span of TRAFFIC_WINDOW_S that
    // contains ≥ TRAFFIC_MIN_CROSSINGS entries is a cluster. Emit one
    // event per cluster, centered on the window.
    let mut out = Vec::new();
    let mut i = 0;
    while i < stop_ends.len() {
        let start = stop_ends[i];
        // Advance j until window exceeds TRAFFIC_WINDOW_S.
        let mut j = i;
        while j < stop_ends.len() && stop_ends[j] - start <= TRAFFIC_WINDOW_S {
            j += 1;
        }
        let count = j - i;
        if count >= TRAFFIC_MIN_CROSSINGS {
            let center = (stop_ends[i] + stop_ends[j - 1]) / 2.0;
            out.push(center);
            // Skip past this cluster's endpoints so we don't emit
            // overlapping cluster events for the same congestion.
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Shortest signed angular delta from `a` to `b` in degrees, in the
/// range `(-180, 180]`. Handles 0°/360° wrap correctly.
fn angular_delta(a: f64, b: f64) -> f64 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Merge overlapping / touching windows in-place. Assumes input is
/// already sorted by `start_s`. Leaves `windows` as a disjoint list.
fn merge_overlapping(windows: &mut Vec<EventWindow>) {
    if windows.is_empty() {
        return;
    }
    windows.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut merged: Vec<EventWindow> = Vec::with_capacity(windows.len());
    for w in windows.iter() {
        if let Some(last) = merged.last_mut() {
            if w.start_s <= last.end_s {
                last.end_s = last.end_s.max(w.end_s);
                continue;
            }
        }
        merged.push(*w);
    }
    *windows = merged;
}

// ── Calibration / diagnostic harness ────────────────────────────────
//
// The test below replays a real trip's stitched GPS against every
// detector and dumps the raw triggering conditions so we can calibrate
// thresholds against ground truth instead of guesses. Marked #[ignore]
// so `cargo test` stays fast; run explicitly:
//
//   cargo test --manifest-path src-tauri/Cargo.toml \
//       --lib -- --ignored --nocapture \
//       timelapse::events::diagnostics::diagnose_12_33_trip
#[cfg(test)]
mod diagnostics {
    use super::*;
    use crate::metadata::mp4_probe;
    use crate::scan::naming::CameraKind;
    use std::path::Path;

    /// The 12 front-channel files of the 2026-03-23 12:33 PM trip,
    /// in time order. Same set the timelapse worker concat-ed.
    const TRIP_12_33_FILES: &[&str] = &[
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_123342_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_123642_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_123942_02_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_124049_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_124349_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_124649_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_124949_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_125249_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_125342_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_125355_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_125655_00_F.MP4",
        r"E:\Wolfbox Dashcam\Videos\2026_03_23_125914_00_F.MP4",
    ];

    fn stitch_trip(files: &[&str]) -> Vec<GpsPoint> {
        let mut out = Vec::new();
        let mut cursor = 0.0;
        for path_str in files {
            let path = Path::new(path_str);
            let dur = mp4_probe::probe(path)
                .map(|m| m.duration_s)
                .unwrap_or(0.0);
            let points = crate::gps::extract_for_kind(path, CameraKind::WolfBox)
                .unwrap_or_default();
            let local_count = points.len();
            for p in points {
                out.push(GpsPoint {
                    t_offset_s: cursor + p.t_offset_s,
                    ..p
                });
            }
            eprintln!(
                "  [{:>6.1} s] {:>4} pts  {}",
                cursor,
                local_count,
                Path::new(path_str)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
            cursor += dur;
        }
        eprintln!("Stitched: {} points over {:.1} s", out.len(), cursor);
        out
    }

    /// Convert concat-seconds to mm:ss for easy cross-reference
    /// against the playback scrubber the user sees.
    fn fmt_mmss(t: f64) -> String {
        let m = (t / 60.0).floor() as i64;
        let s = t - (m as f64) * 60.0;
        format!("{m}:{s:04.1}")
    }

    /// Speed at the point nearest to concat-time `t`, or 0 if no data.
    fn speed_near(points: &[GpsPoint], t: f64) -> f64 {
        points
            .iter()
            .filter(|p| p.fix_ok)
            .min_by(|a, b| {
                (a.t_offset_s - t)
                    .abs()
                    .partial_cmp(&(b.t_offset_s - t).abs())
                    .unwrap()
            })
            .map(|p| p.speed_mps)
            .unwrap_or(0.0)
    }

    fn mps_to_mph(v: f64) -> f64 {
        v * 2.23693629
    }

    #[test]
    #[ignore]
    fn diagnose_12_33_trip() {
        eprintln!("\n=== Loading 12:33 PM trip ===");
        let gps = stitch_trip(TRIP_12_33_FILES);

        // Dump every brake candidate with its context regardless of
        // threshold, so we can see how marginal each trigger is.
        eprintln!("\n=== Raw deceleration / acceleration events ===");
        eprintln!(
            "Thresholds: brake ≤ {:.1} m/s², accel ≥ {:+.1} m/s² (current)",
            HARD_BRAKE_MPS2, HARD_ACCEL_MPS2
        );
        eprintln!(
            "{:>10}  {:>8}  {:>8}  {:>8}  {:>10}  {}",
            "time", "dv/dt", "v-before", "v-after", "|Δh|/s", "kind"
        );
        for pair in gps.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if !a.fix_ok || !b.fix_ok {
                continue;
            }
            let dt = b.t_offset_s - a.t_offset_s;
            if dt <= 0.0 || dt > 5.0 {
                continue;
            }
            let dv = (b.speed_mps - a.speed_mps) / dt;
            let heading_rate = angular_delta(a.heading_deg, b.heading_deg).abs() / dt;

            let kind = if dv <= HARD_BRAKE_MPS2 {
                "HARD_BRAKE"
            } else if dv >= HARD_ACCEL_MPS2 {
                "HARD_ACCEL"
            } else if dv.abs() >= 2.0 {
                "(moderate)"
            } else {
                ""
            };
            if !kind.is_empty() {
                eprintln!(
                    "  {:>10}  {:>+8.2}  {:>5.1} mph {:>4.1} mph  {:>8.1}°/s  {}",
                    fmt_mmss(b.t_offset_s),
                    dv,
                    mps_to_mph(a.speed_mps),
                    mps_to_mph(b.speed_mps),
                    heading_rate,
                    kind,
                );
            }
        }

        eprintln!("\n=== Sharp-turn candidates (any ≥ 20°/s) ===");
        let mut prev_exceed = false;
        for pair in gps.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if !a.fix_ok || !b.fix_ok {
                prev_exceed = false;
                continue;
            }
            let dt = b.t_offset_s - a.t_offset_s;
            if dt <= 0.0 || dt > 5.0 {
                prev_exceed = false;
                continue;
            }
            let rate = angular_delta(a.heading_deg, b.heading_deg).abs() / dt;
            if rate >= 20.0 {
                let sustained = prev_exceed && rate >= SHARP_TURN_DEG_PER_S;
                eprintln!(
                    "  {:>10}  rate={:>6.1}°/s  v={:>4.1} mph  {}",
                    fmt_mmss(b.t_offset_s),
                    rate,
                    mps_to_mph(b.speed_mps),
                    if sustained { "TRIGGERS" } else { "" },
                );
                prev_exceed = rate >= SHARP_TURN_DEG_PER_S;
            } else {
                prev_exceed = false;
            }
        }

        eprintln!("\n=== Stops ≥ 10 s (threshold for long-stop: {} s) ===", LONG_STOP_MIN_S);
        let mut run_start: Option<f64> = None;
        for p in &gps {
            if !p.fix_ok {
                continue;
            }
            if p.speed_mps < LONG_STOP_MPS {
                if run_start.is_none() {
                    run_start = Some(p.t_offset_s);
                }
            } else if let Some(start) = run_start.take() {
                let len = p.t_offset_s - start;
                if len >= 10.0 {
                    let flag = if len >= LONG_STOP_MIN_S {
                        "LONG_STOP TRIGGERS"
                    } else {
                        ""
                    };
                    eprintln!(
                        "  {:>10}..{:<10}  duration={:>5.1} s  {}",
                        fmt_mmss(start),
                        fmt_mmss(p.t_offset_s),
                        len,
                        flag,
                    );
                }
            }
        }
        if let (Some(start), Some(last)) = (run_start, gps.last()) {
            let len = last.t_offset_s - start;
            if len >= 10.0 {
                let flag = if len >= LONG_STOP_MIN_S {
                    "LONG_STOP TRIGGERS (to end of trip)"
                } else {
                    "(to end of trip)"
                };
                eprintln!(
                    "  {:>10}..{:<10}  duration={:>5.1} s  {}",
                    fmt_mmss(start),
                    fmt_mmss(last.t_offset_s),
                    len,
                    flag,
                );
            }
        }

        eprintln!("\n=== Windows produced by current detect_events ===");
        let windows = detect_events(&gps);
        eprintln!("Total: {} windows", windows.len());
        for w in &windows {
            eprintln!(
                "  {:>10} – {:<10}  duration={:>5.1} s   v≈{:>4.1} mph at center",
                fmt_mmss(w.start_s),
                fmt_mmss(w.end_s),
                w.end_s - w.start_s,
                mps_to_mph(speed_near(&gps, (w.start_s + w.end_s) / 2.0)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(t: f64, speed: f64, heading: f64) -> GpsPoint {
        GpsPoint {
            t_offset_s: t,
            lat: 0.0,
            lon: 0.0,
            speed_mps: speed,
            heading_deg: heading,
            altitude_m: 0.0,
            fix_ok: true,
        }
    }

    // ── angular_delta ──────────────────────────────────────────────────

    #[test]
    fn angular_delta_handles_wrap_around() {
        assert!((angular_delta(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert!((angular_delta(10.0, 350.0) - -20.0).abs() < 1e-9);
        assert!((angular_delta(0.0, 180.0) - 180.0).abs() < 1e-9);
        // Range: a 181° step should come back as -179°.
        assert!((angular_delta(0.0, 181.0) - -179.0).abs() < 1e-9);
    }

    // ── speed spikes ───────────────────────────────────────────────────

    #[test]
    fn hard_brake_is_detected() {
        // -4.5 m/s²; speed after (15.5) well above MOVING_MPS.
        let gps = vec![pt(0.0, 20.0, 0.0), pt(1.0, 15.5, 0.0)];
        let events = detect_speed_spikes(&gps);
        assert_eq!(events, vec![1.0]);
    }

    #[test]
    fn gentle_decel_is_not_detected() {
        let gps = vec![pt(0.0, 20.0, 0.0), pt(1.0, 19.0, 0.0)]; // -1 m/s²
        assert!(detect_speed_spikes(&gps).is_empty());
    }

    #[test]
    fn brake_to_stop_is_filtered() {
        // -6 m/s² deceleration but ending stopped: routine traffic
        // braking at a light, not an interesting moment.
        let gps = vec![pt(0.0, 6.0, 0.0), pt(1.0, 0.0, 0.0)];
        assert!(
            detect_speed_spikes(&gps).is_empty(),
            "brake that ends stopped must be filtered"
        );
    }

    #[test]
    fn emergency_brake_mid_motion_still_fires_even_if_it_ends_low() {
        // Peak deceleration caught mid-brake at 10 m/s → 5 m/s. Both
        // endpoints are above MOVING_MPS, so the event fires even if
        // the driver eventually comes to a stop after further braking.
        let gps = vec![pt(0.0, 10.0, 0.0), pt(1.0, 5.0, 0.0)]; // -5 m/s²
        assert_eq!(detect_speed_spikes(&gps), vec![1.0]);
    }

    #[test]
    fn hard_accel_is_detected() {
        // +4 m/s² with speed-before (5) above MOVING_MPS — real mid-
        // motion acceleration, not a pull-from-stop.
        let gps = vec![pt(0.0, 5.0, 0.0), pt(1.0, 9.0, 0.0)];
        assert_eq!(detect_speed_spikes(&gps), vec![1.0]);
    }

    #[test]
    fn accel_from_stop_is_filtered() {
        // Pulling away from a light at 4 m/s² — routine. Even though
        // dv/dt passes threshold, speed-before is 0, so filter it.
        let gps = vec![pt(0.0, 0.0, 0.0), pt(1.0, 4.0, 0.0)];
        assert!(
            detect_speed_spikes(&gps).is_empty(),
            "accel from a stop must be filtered"
        );
    }

    #[test]
    fn fix_loss_suppresses_spike_detection() {
        let mut a = pt(0.0, 20.0, 0.0);
        a.fix_ok = false;
        let gps = vec![a, pt(1.0, 10.0, 0.0)];
        assert!(detect_speed_spikes(&gps).is_empty());
    }

    // ── sharp turns ────────────────────────────────────────────────────

    #[test]
    fn sharp_turn_needs_sustained_rate() {
        // One 40°/s sample isn't enough; need two in a row ≥ 1 s apart.
        let one_sample = vec![pt(0.0, 10.0, 0.0), pt(1.0, 10.0, 40.0)];
        assert!(detect_sharp_turns(&one_sample).is_empty());

        let two_samples = vec![
            pt(0.0, 10.0, 0.0),
            pt(1.0, 10.0, 40.0),
            pt(2.0, 10.0, 80.0),
        ];
        assert_eq!(detect_sharp_turns(&two_samples), vec![2.0]);
    }

    #[test]
    fn sharp_turn_handles_heading_wrap() {
        // Two samples of -40°/s each via 0/360 boundary.
        let gps = vec![
            pt(0.0, 10.0, 10.0),
            pt(1.0, 10.0, 330.0), // effective Δ: -40°
            pt(2.0, 10.0, 290.0), // another -40°
        ];
        assert_eq!(detect_sharp_turns(&gps), vec![2.0]);
    }

    #[test]
    fn sharp_turn_at_zero_speed_is_filtered() {
        // Stationary vehicle with wildly jittering heading — typical
        // GPS noise while parked at a light. Should NOT trigger any
        // sharp-turn events.
        let gps = vec![
            pt(0.0, 0.0, 0.0),
            pt(1.0, 0.0, 90.0),   // 90°/s heading change
            pt(2.0, 0.0, 200.0),  // another 110°/s
            pt(3.0, 0.0, 330.0),  // another 130°/s
        ];
        assert!(
            detect_sharp_turns(&gps).is_empty(),
            "heading jitter at zero speed must not trigger sharp-turn events"
        );
    }

    // ── long stops ─────────────────────────────────────────────────────

    #[test]
    fn long_stop_is_detected_at_midpoint() {
        // 140s of stop → event at t=70.
        let mut gps = Vec::new();
        for t in 0..=140 {
            gps.push(pt(t as f64, 0.0, 0.0));
        }
        gps.push(pt(141.0, 5.0, 0.0)); // resume motion
        assert_eq!(detect_long_stops(&gps), vec![70.5]);
    }

    #[test]
    fn short_stop_is_not_flagged() {
        let mut gps = Vec::new();
        for t in 0..=30 {
            gps.push(pt(t as f64, 0.0, 0.0)); // only 30s stopped
        }
        gps.push(pt(31.0, 5.0, 0.0));
        assert!(detect_long_stops(&gps).is_empty());
    }

    #[test]
    fn stop_that_runs_to_trip_end_still_flags() {
        let mut gps = vec![pt(0.0, 5.0, 0.0)];
        for t in 1..=130 {
            gps.push(pt(t as f64, 0.0, 0.0));
        }
        // No resume; run ends at t=130 → midpoint 65.5.
        let events = detect_long_stops(&gps);
        assert_eq!(events.len(), 1);
        assert!((events[0] - 65.5).abs() < 1e-9);
    }

    // ── traffic clusters ───────────────────────────────────────────────

    #[test]
    fn traffic_cluster_detects_five_stops_in_a_minute() {
        // Alternating stop/go, each phase 5s, for 5 full cycles.
        let mut gps = Vec::new();
        let mut t = 0.0;
        for _ in 0..5 {
            for _ in 0..5 {
                gps.push(pt(t, 0.0, 0.0)); // stopped
                t += 1.0;
            }
            for _ in 0..5 {
                gps.push(pt(t, 5.0, 0.0)); // moving
                t += 1.0;
            }
        }
        let events = detect_traffic_clusters(&gps);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn traffic_without_enough_crossings_not_detected() {
        // Only 3 crossings in 60s.
        let gps = vec![
            pt(0.0, 0.0, 0.0),
            pt(5.0, 5.0, 0.0), // crossing 1
            pt(10.0, 0.0, 0.0),
            pt(15.0, 5.0, 0.0), // crossing 2
            pt(20.0, 0.0, 0.0),
            pt(25.0, 5.0, 0.0), // crossing 3
            pt(30.0, 0.0, 0.0),
        ];
        assert!(detect_traffic_clusters(&gps).is_empty());
    }

    // ── end-to-end detect_events ───────────────────────────────────────

    #[test]
    fn detect_events_expands_and_merges_windows() {
        // Two hard brakes 8s apart → windows overlap and merge.
        let gps = vec![
            pt(0.0, 20.0, 0.0),
            pt(1.0, 15.0, 0.0), // brake at t=1 → window [0, 11]
            pt(2.0, 15.0, 0.0),
            pt(3.0, 15.0, 0.0),
            pt(4.0, 15.0, 0.0),
            pt(5.0, 15.0, 0.0),
            pt(6.0, 15.0, 0.0),
            pt(7.0, 15.0, 0.0),
            pt(8.0, 15.0, 0.0),
            pt(9.0, 4.0, 0.0), // brake at t=9 → window [4, 19]
        ];
        let windows = detect_events(&gps);
        assert_eq!(windows.len(), 1, "adjacent windows must merge");
        assert!((windows[0].start_s - 0.0).abs() < 1e-9);
        assert!((windows[0].end_s - 19.0).abs() < 1e-9);
    }

    #[test]
    fn detect_events_clamps_window_start_at_zero() {
        let gps = vec![pt(0.0, 20.0, 0.0), pt(1.0, 16.0, 0.0)];
        let windows = detect_events(&gps);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].start_s, 0.0);
        // Event at t=1, trail 10s → 11s.
        assert!((windows[0].end_s - 11.0).abs() < 1e-9);
    }

    #[test]
    fn empty_gps_returns_no_windows() {
        assert!(detect_events(&[]).is_empty());
    }

    // ── merge_overlapping ──────────────────────────────────────────────

    #[test]
    fn merge_sorts_and_coalesces() {
        let mut w = vec![
            EventWindow { start_s: 10.0, end_s: 20.0 },
            EventWindow { start_s: 0.0, end_s: 5.0 },
            EventWindow { start_s: 15.0, end_s: 25.0 },
            EventWindow { start_s: 30.0, end_s: 35.0 },
        ];
        merge_overlapping(&mut w);
        assert_eq!(
            w,
            vec![
                EventWindow { start_s: 0.0, end_s: 5.0 },
                EventWindow { start_s: 10.0, end_s: 25.0 },
                EventWindow { start_s: 30.0, end_s: 35.0 },
            ]
        );
    }
}
