//! VIOFO filename recognition.
//!
//! A229-family cameras store each camera channel in a separate file. The
//! optional per-file sequence number is monotonic across channels, so the three files
//! belonging to one recording have *different* sequence numbers. Grouping must
//! therefore use the recording timestamp (plus parking-mode marker), not the
//! sequence number.
//!
//! Observed / documented shapes:
//!   `2025_0706_201953_F.MP4`       front without a sequence counter
//!   `2025_0706_201953_I.MP4`       interior without a sequence counter
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
    if tail.is_empty() {
        return None;
    }

    // Match the ASCII channel first, before slicing a potentially non-ASCII
    // filename. This also avoids panicking on an unrecognized Unicode suffix.
    let channel_label = match tail.as_bytes().last()?.to_ascii_uppercase() {
        b'F' => LABEL_FRONT.to_string(),
        b'I' => LABEL_INTERIOR.to_string(),
        b'R' => LABEL_REAR.to_string(),
        _ => return None,
    };
    let seq_and_mode = &tail[..tail.len() - 1];

    // Parking recordings insert `P` immediately before the camera letter.
    // The counter is optional, and its width is not a firmware-version test.
    let (serial, parking) = if seq_and_mode.ends_with(['P', 'p']) {
        (&seq_and_mode[..seq_and_mode.len() - 1], true)
    } else {
        (seq_and_mode, false)
    };
    if !serial.bytes().all(|b| b.is_ascii_digit()) {
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
    let (stem, extension) = filename.rsplit_once('.')?;
    extension.eq_ignore_ascii_case("mp4").then_some(stem)
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
        assert!(parse("2026_08_31_065229_00_F.MP4").is_none());
    }

    #[test]
    fn accepts_sequence_free_names_and_preserves_the_timestamp_group_key() {
        for (suffix, label) in [("F", LABEL_FRONT), ("I", LABEL_INTERIOR), ("R", LABEL_REAR)] {
            let plain = parse(&format!("2025_0706_201953_{suffix}.MP4")).unwrap();
            let numbered = parse(&format!("2025_0706_201953_000123{suffix}.MP4")).unwrap();
            assert_eq!(plain.channel_label, label);
            assert_eq!(plain.group_key, numbered.group_key);
            assert_eq!(plain.start_time, numbered.start_time);
            assert_eq!(plain.start_time.to_string(), "2025-07-06 20:19:53");
        }
    }

    #[test]
    fn detects_counter_and_parking_independently_for_each_file() {
        for serial in ["", "0", "012", "000001", "12345678901234567890"] {
            for channel in ["F", "I", "R"] {
                for (marker, mode) in [("", "N"), ("P", "P")] {
                    let parsed = parse(&format!("2024_0229_235959_{serial}{marker}{channel}.MP4")).unwrap();
                    assert_eq!(parsed.group_key, format!("vf:2024_0229_235959_{mode}"));
                }
            }
        }
        assert_eq!(parse("2024_0229_235959_pf.mP4").unwrap().group_key, "vf:2024_0229_235959_P");
    }

    #[test]
    fn rejects_malformed_dates_modes_channels_and_non_ascii_suffixes_without_panicking() {
        for name in ["2025_0229_235959_F.MP4", "2025_0706_240000_F.MP4",
            "2025_0706_201953_.MP4", "2025_0706_201953_P.MP4", "2025_0706_201953_X.MP4",
            "2025_0706_201953_P12F.MP4", "2025_0706_201953_12PPF.MP4",
            "2025_0706_201953_12_F.MP4", "2025_0706_201953_é.MP4",
            "2025_0706_201953_éF.MP4", "2025_0706_201953_F.MOV"] {
            assert!(parse(name).is_none(), "unexpected match for {name}");
        }
    }
}
