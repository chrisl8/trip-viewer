//! Place validated VIOFO GPS UTC observations on the local video timeline.
//!
//! VIOFO A229-family filenames contain the camera's local wall-clock start
//! time, while Novatek GPS records contain UTC. Validation of the raw GPS data
//! is intentionally performed elsewhere and never depends on a timezone.
//!
//! This module currently retains the existing best-effort civil UTC-offset
//! inference solely for synchronization. Keeping it isolated makes a later
//! trip-level camera-clock model possible without coupling clock assumptions to
//! the reusable consistency validator.

use chrono::{Duration, NaiveDateTime};
use std::path::Path;

const MIN_UTC_OFFSET_MINUTES: i64 = -12 * 60;
const MAX_UTC_OFFSET_MINUTES: i64 = 14 * 60;
const UTC_OFFSET_STEP_MINUTES: i64 = 15;
const ALIGNMENT_TOLERANCE_S: f64 = 2.0;

/// Return the video-relative time of the first already-validated VIOFO GPS fix.
///
/// The caller supplies the measured clip duration and the first UTC timestamp
/// that survived consistency validation. This avoids reparsing the MP4 and,
/// more importantly, prevents malformed raw records from becoming timing
/// anchors.
pub fn first_fix_delay_s(
    path: &Path,
    first_gps_utc: NaiveDateTime,
    duration_s: f64,
) -> Option<f64> {
    let filename = path.file_name().and_then(|n| n.to_str())?;
    let parsed = crate::scan::viofo::parse(filename)?;

    let Some((utc_offset_minutes, delay_s)) =
        infer_alignment(parsed.start_time, first_gps_utc, duration_s)
    else {
        eprintln!(
            "viofo gps: could not uniquely align validated GPS UTC time to video start in {}",
            path.display()
        );
        return None;
    };

    if delay_s > ALIGNMENT_TOLERANCE_S {
        let sign = if utc_offset_minutes >= 0 { '+' } else { '-' };
        let abs_minutes = utc_offset_minutes.abs();
        eprintln!(
            "viofo gps: first validated fix at +{delay_s:.1}s in {} (camera UTC{sign}{:02}:{:02})",
            path.display(),
            abs_minutes / 60,
            abs_minutes % 60,
        );
    }

    Some(delay_s)
}

fn infer_alignment(
    video_start_local: NaiveDateTime,
    first_gps_utc: NaiveDateTime,
    duration_s: f64,
) -> Option<(i64, f64)> {
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return None;
    }

    let mut candidates = Vec::new();
    let mut offset_minutes = MIN_UTC_OFFSET_MINUTES;
    while offset_minutes <= MAX_UTC_OFFSET_MINUTES {
        let gps_as_local = first_gps_utc + Duration::minutes(offset_minutes);
        let delay_s = (gps_as_local - video_start_local).num_milliseconds() as f64 / 1000.0;

        if delay_s >= -ALIGNMENT_TOLERANCE_S && delay_s <= duration_s + ALIGNMENT_TOLERANCE_S {
            candidates.push((offset_minutes, delay_s.max(0.0)));
        }

        offset_minutes += UTC_OFFSET_STEP_MINUTES;
    }

    if candidates.len() == 1 {
        candidates.into_iter().next()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, s)
            .unwrap()
    }

    #[test]
    fn aligns_real_late_fix_example() {
        let aligned = infer_alignment(
            dt(2026, 8, 31, 18, 37, 41),
            dt(2026, 8, 31, 16, 39, 8),
            180.0,
        );
        assert_eq!(aligned, Some((120, 87.0)));
    }

    #[test]
    fn aligns_immediate_fix_with_timezone() {
        let aligned = infer_alignment(
            dt(2026, 8, 31, 6, 52, 29),
            dt(2026, 8, 31, 4, 52, 29),
            180.0,
        );
        assert_eq!(aligned, Some((120, 0.0)));
    }

    #[test]
    fn supports_quarter_hour_civil_offsets() {
        let aligned = infer_alignment(dt(2026, 8, 31, 9, 59, 30), dt(2026, 8, 31, 4, 15, 0), 180.0);
        assert_eq!(aligned, Some((345, 30.0)));
    }

    #[test]
    fn refuses_ambiguous_long_clip_alignment() {
        let aligned = infer_alignment(dt(2026, 8, 31, 10, 0, 0), dt(2026, 8, 31, 8, 0, 0), 1800.0);
        assert_eq!(aligned, None);
    }
}
