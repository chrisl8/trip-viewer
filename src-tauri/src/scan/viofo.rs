//! VIOFO filename recognition.
//!
//! A229-family cameras store each camera channel in a separate file. The
//! per-file sequence number is monotonic across channels, so the three files
//! belonging to one recording have *different* sequence numbers. Grouping must
//! therefore use the recording timestamp (plus parking-mode marker), not the
//! sequence number.
//!
//! Observed / documented shapes:
//!   `2026_0831_065229_000646F.MP4`  normal front
//!   `2026_0831_065229_000647I.MP4`  normal interior
//!   `2026_0831_065229_000648R.MP4`  normal rear
//!   `2023_0821_180010_062PF.MP4`    parking front
//!   `2023_0821_180010_063PI.MP4`    parking interior
//!   `2023_0821_180010_064PR.MP4`    parking rear

use crate::model::{LABEL_FRONT, LABEL_INTERIOR, LABEL_REAR};
use crate::scan::naming::{CameraKind, EventMode, ParsedName};
use chrono::NaiveDateTime;

/// Parse a VIOFO A229-family filename.
///
/// Until `CameraKind` grows a dedicated VIOFO variant, VIOFO files use the
/// existing `Generic` kind. GPS dispatch distinguishes VIOFO by filename and
/// routes it to the Novatek decoder, so this is functionally brand-aware while
/// keeping the change isolated from persisted enum values.
pub fn parse(filename: &str) -> Option<ParsedName> {
    let stem = strip_mp4_ext(filename)?;
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() != 4 {
        return None;
    }

    let year = parts[0];
    let mmdd = parts[1];
    let hms = parts[2];
    let tail = parts[3];

    if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if mmdd.len() != 4 || !mmdd.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if hms.len() != 6 || !hms.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if tail.len() < 2 {
        return None;
    }

    let (seq_and_mode, channel) = tail.split_at(tail.len() - 1);
    let channel_label = match channel {
        "F" => LABEL_FRONT.to_string(),
        "I" => LABEL_INTERIOR.to_string(),
        "R" => LABEL_REAR.to_string(),
        _ => return None,
    };

    // Parking recordings insert `P` immediately before the camera letter.
    // Sequence number width has changed across firmware generations, so accept
    // any non-empty run of decimal digits rather than hard-coding 3/5/6 digits.
    let (serial, parking) = match seq_and_mode.strip_suffix('P') {
        Some(serial) => (serial, true),
        None => (seq_and_mode, false),
    };
    if serial.is_empty() || !serial.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let dt = format!("{year}{mmdd}{hms}");
    let start_time = NaiveDateTime::parse_from_str(&dt, "%Y%m%d%H%M%S").ok()?;

    // The sequence number differs across F/I/R, so intentionally exclude it.
    // Parking and normal clips at an identical clock second must remain
    // distinct, hence the final mode marker in the key.
    let mode = if parking { "P" } else { "N" };
    let group_key = format!("vf:{year}_{mmdd}_{hms}_{mode}");

    Some(ParsedName {
        start_time,
        event_mode: EventMode::Normal,
        channel_label,
        group_key,
        camera_kind: CameraKind::Generic,
    })
}

pub fn is_viofo_filename(filename: &str) -> bool {
    parse(filename).is_some()
}

fn strip_mp4_ext(filename: &str) -> Option<&str> {
    filename
        .strip_suffix(".MP4")
        .or_else(|| filename.strip_suffix(".mp4"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parses_a229_pro_normal_triplet() {
        let f = parse("2026_0831_065229_000646F.MP4").unwrap();
        let i = parse("2026_0831_065229_000647I.MP4").unwrap();
        let r = parse("2026_0831_065229_000648R.MP4").unwrap();

        assert_eq!(f.channel_label, LABEL_FRONT);
        assert_eq!(i.channel_label, LABEL_INTERIOR);
        assert_eq!(r.channel_label, LABEL_REAR);
        assert_eq!(f.group_key, i.group_key);
        assert_eq!(i.group_key, r.group_key);
        assert_eq!(f.camera_kind, CameraKind::Generic);
        assert_eq!(
            f.start_time,
            NaiveDate::from_ymd_opt(2026, 8, 31)
                .unwrap()
                .and_hms_opt(6, 52, 29)
                .unwrap()
        );
    }

    #[test]
    fn parses_documented_parking_triplet() {
        let f = parse("2023_0821_180010_062PF.MP4").unwrap();
        let i = parse("2023_0821_180010_063PI.MP4").unwrap();
        let r = parse("2023_0821_180010_064PR.MP4").unwrap();
        assert_eq!(f.group_key, i.group_key);
        assert_eq!(i.group_key, r.group_key);
        assert!(f.group_key.ends_with("_P"));
    }

    #[test]
    fn normal_and_parking_do_not_collide() {
        let normal = parse("2023_0821_180010_000062F.MP4").unwrap();
        let parking = parse("2023_0821_180010_062PF.MP4").unwrap();
        assert_ne!(normal.group_key, parking.group_key);
    }

    #[test]
    fn accepts_sequence_number_width_changes() {
        assert!(parse("2024_0827_112625_00001F.MP4").is_some());
        assert!(parse("2023_0821_180010_062F.MP4").is_some());
        assert!(parse("2026_0831_065229_000646F.mp4").is_some());
    }

    #[test]
    fn rejects_near_misses() {
        assert!(parse("2026_0831_065229_000646X.MP4").is_none());
        assert!(parse("2026_1331_065229_000646F.MP4").is_none());
        assert!(parse("2026_0831_256229_000646F.MP4").is_none());
        assert!(parse("2026_0831_065229_PF.MP4").is_none());
        assert!(parse("2026_08_31_065229_00_F.MP4").is_none());
    }
}
