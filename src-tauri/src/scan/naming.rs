//! Filename parsing with auto-detection across multiple dashcam formats.
//!
//! We try each parser in order (Wolf Box → Thinkware → Generic4Channel)
//! and use the first one that recognizes the filename. This lets the app
//! accept footage from any supported dashcam without the user having to
//! configure anything or rename files.

use crate::error::AppError;
use crate::model::{LABEL_FRONT, LABEL_INTERIOR, LABEL_REAR};
use chrono::NaiveDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventMode {
    Normal,
    Event,
    Other(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub start_time: NaiveDateTime,
    pub event_mode: EventMode,
    /// Free-form channel label ("Front", "Rear", "Channel A", etc.).
    pub channel_label: String,
    /// All channels of the same segment share this key. Used for grouping.
    pub group_key: String,
}

/// A filename-format recognizer. Each parser knows one vendor's convention
/// (or a generic fallback). Returns `None` if the filename doesn't match
/// this format.
trait FilenameParser: Send + Sync {
    fn parse(&self, filename: &str) -> Option<ParsedName>;
}

/// Try each parser in order. The first one to match wins.
/// Returns `Err(InvalidFilename)` if no parser matches.
pub fn parse(filename: &str) -> Result<ParsedName, AppError> {
    for parser in parsers() {
        if let Some(p) = parser.parse(filename) {
            return Ok(p);
        }
    }
    Err(AppError::InvalidFilename(filename.into()))
}

fn parsers() -> Vec<Box<dyn FilenameParser>> {
    // Order matters: put the most specific formats first, most generic last.
    // Wolf Box and Thinkware have very distinct shapes so they can't conflict;
    // the generic 4-channel parser runs last so it only catches leftovers.
    vec![
        Box::new(WolfBoxParser),
        Box::new(ThinkwareParser),
        Box::new(Generic4ChannelParser),
    ]
}

fn strip_mp4(filename: &str) -> Option<&str> {
    filename
        .strip_suffix(".MP4")
        .or_else(|| filename.strip_suffix(".mp4"))
}

// ── Wolf Box ────────────────────────────────────────────────────────────────
//
// Format: `YYYY_MM_DD_HHMMSS_EE_C.MP4`
// Example: `2026_03_15_173951_02_F.MP4`
//   EE = event code (00 = Normal, 02 = Event, other 2-digit values allowed)
//   C  = channel letter (F=Front, I=Interior, R=Rear)

struct WolfBoxParser;

impl FilenameParser for WolfBoxParser {
    fn parse(&self, filename: &str) -> Option<ParsedName> {
        let stem = strip_mp4(filename)?;
        let parts: Vec<&str> = stem.split('_').collect();
        if parts.len() != 6 {
            return None;
        }

        let (year, month, day, hms, event_code, chan) =
            (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]);

        let dt_str = format!("{year}_{month}_{day}_{hms}");
        let start_time =
            NaiveDateTime::parse_from_str(&dt_str, "%Y_%m_%d_%H%M%S").ok()?;

        let event_mode = match event_code {
            "00" => EventMode::Normal,
            "02" => EventMode::Event,
            other => EventMode::Other(other.parse().ok()?),
        };

        let channel_label = match chan {
            "F" => LABEL_FRONT.to_string(),
            "I" => LABEL_INTERIOR.to_string(),
            "R" => LABEL_REAR.to_string(),
            _ => return None,
        };

        let group_key = format!("wb:{year}_{month}_{day}_{hms}_{event_code}");

        Some(ParsedName {
            start_time,
            event_mode,
            channel_label,
            group_key,
        })
    }
}

// ── Thinkware ───────────────────────────────────────────────────────────────
//
// Format: `XXX_YYYY_MM_DD_HH_MM_SS_C.MP4`
// Example: `REC_2026_03_06_07_25_52_F.MP4`
//   C = channel letter (F=Front, R=Rear; Thinkware F200 Pro is 2-channel)
//
// Known 3-letter prefixes:
//   REC — continuous driving (cont_rec/ folder)
//   EVT — g-sensor event (evt_rec/ folder)
//   MAN — manual user-triggered (manual_rec/ folder)
//   Parking and motion-timelapse prefixes are unconfirmed. The filename
//   shape is distinctive enough that any 3-letter uppercase prefix is
//   safely assumed to be Thinkware; unknown prefixes default to Normal
//   event mode until we learn what Thinkware uses.

struct ThinkwareParser;

impl FilenameParser for ThinkwareParser {
    fn parse(&self, filename: &str) -> Option<ParsedName> {
        let stem = strip_mp4(filename)?;
        let parts: Vec<&str> = stem.split('_').collect();
        if parts.len() != 8 {
            return None;
        }

        let (prefix, year, month, day, hh, mm, ss, chan) =
            (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5], parts[6], parts[7]);

        // Prefix must be 3 uppercase ASCII letters. This keeps us from
        // eagerly matching unrelated formats that happen to have 8 parts.
        if prefix.len() != 3 || !prefix.chars().all(|c| c.is_ascii_uppercase()) {
            return None;
        }

        let event_mode = match prefix {
            "EVT" => EventMode::Event,
            // REC, MAN, and unknown prefixes (parking, motion-timelapse)
            // all classified as Normal — not g-sensor incidents.
            _ => EventMode::Normal,
        };

        let dt_str = format!("{year}-{month}-{day}T{hh}:{mm}:{ss}");
        let start_time =
            NaiveDateTime::parse_from_str(&dt_str, "%Y-%m-%dT%H:%M:%S").ok()?;

        let channel_label = match chan {
            "F" => LABEL_FRONT.to_string(),
            "R" => LABEL_REAR.to_string(),
            _ => return None,
        };

        let group_key = format!("tw:{year}{month}{day}_{hh}{mm}{ss}_{prefix}");

        Some(ParsedName {
            start_time,
            event_mode,
            channel_label,
            group_key,
        })
    }
}

// ── Generic 4-channel fallback ──────────────────────────────────────────────
//
// Best-effort catch-all for 4-channel dashcams. Looks for a timestamp anywhere
// in the filename followed by a single channel letter (A/B/C/D) or digit (1-4)
// as the last underscore-separated component before the extension.
//
// Example formats that should match:
//   `2026_03_06_072552_A.MP4`
//   `2026_03_06_072552_1.MP4`
//   `CAM_2026_03_06_072552_B.MP4`
//
// No real sample files yet — this will be tuned when a 4-channel user tries it.

struct Generic4ChannelParser;

impl FilenameParser for Generic4ChannelParser {
    fn parse(&self, filename: &str) -> Option<ParsedName> {
        let stem = strip_mp4(filename)?;
        let parts: Vec<&str> = stem.split('_').collect();
        if parts.len() < 2 {
            return None;
        }

        // Channel suffix: last part must be a single char A-D or 1-4.
        let chan = parts.last()?;
        let channel_label = match *chan {
            "A" | "a" => "Channel A".to_string(),
            "B" | "b" => "Channel B".to_string(),
            "C" | "c" => "Channel C".to_string(),
            "D" | "d" => "Channel D".to_string(),
            "1" => "Channel 1".to_string(),
            "2" => "Channel 2".to_string(),
            "3" => "Channel 3".to_string(),
            "4" => "Channel 4".to_string(),
            _ => return None,
        };

        // Look for a date + time in the earlier parts. Accept two shapes:
        //   YYYY MM DD HHMMSS (4 consecutive parts)
        //   YYYY MM DD HH MM SS (6 consecutive parts)
        let earlier = &parts[..parts.len() - 1];
        let (start_time, ts_key) = find_timestamp(earlier)?;

        let group_key = format!("g4:{ts_key}");

        Some(ParsedName {
            start_time,
            event_mode: EventMode::Normal,
            channel_label,
            group_key,
        })
    }
}

/// Scan `parts` for an embedded timestamp. Returns the parsed time plus
/// a stable key derived from the matched slice (so all channels from the
/// same recording produce the same key).
fn find_timestamp(parts: &[&str]) -> Option<(NaiveDateTime, String)> {
    // Try YYYY_MM_DD_HHMMSS (4 parts in a row).
    for i in 0..parts.len().saturating_sub(3) {
        let window = &parts[i..i + 4];
        let dt_str = format!("{}_{}_{}_{}", window[0], window[1], window[2], window[3]);
        if let Ok(dt) =
            NaiveDateTime::parse_from_str(&dt_str, "%Y_%m_%d_%H%M%S")
        {
            return Some((dt, dt_str));
        }
    }
    // Try YYYY_MM_DD_HH_MM_SS (6 parts in a row).
    for i in 0..parts.len().saturating_sub(5) {
        let window = &parts[i..i + 6];
        let dt_str = format!(
            "{}_{}_{}_{}_{}_{}",
            window[0], window[1], window[2], window[3], window[4], window[5]
        );
        if let Ok(dt) = NaiveDateTime::parse_from_str(&dt_str, "%Y_%m_%d_%H_%M_%S") {
            return Some((dt, dt_str));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    // ── Wolf Box ────────────────────────────────────────────────────────────

    #[test]
    fn parses_normal_front() {
        let p = parse("2026_03_23_094634_00_F.MP4").unwrap();
        assert_eq!(p.channel_label, LABEL_FRONT);
        assert_eq!(p.event_mode, EventMode::Normal);
        assert_eq!(
            p.start_time,
            NaiveDate::from_ymd_opt(2026, 3, 23)
                .unwrap()
                .and_hms_opt(9, 46, 34)
                .unwrap()
        );
        assert!(p.group_key.starts_with("wb:"));
    }

    #[test]
    fn parses_event_interior() {
        let p = parse("2026_03_15_173951_02_I.MP4").unwrap();
        assert_eq!(p.channel_label, LABEL_INTERIOR);
        assert_eq!(p.event_mode, EventMode::Event);
    }

    #[test]
    fn parses_rear_lowercase_extension() {
        let p = parse("2026_04_10_162529_00_R.mp4").unwrap();
        assert_eq!(p.channel_label, LABEL_REAR);
    }

    #[test]
    fn triplet_shares_group_key() {
        let f = parse("2026_03_15_173951_02_F.MP4").unwrap();
        let i = parse("2026_03_15_173951_02_I.MP4").unwrap();
        let r = parse("2026_03_15_173951_02_R.MP4").unwrap();
        assert_eq!(f.group_key, i.group_key);
        assert_eq!(i.group_key, r.group_key);
    }

    #[test]
    fn accepts_other_event_code() {
        let p = parse("2026_03_23_094634_05_F.MP4").unwrap();
        assert_eq!(p.event_mode, EventMode::Other(5));
    }

    // ── Thinkware ──────────────────────────────────────────────────────────

    #[test]
    fn parses_thinkware_rec_front() {
        let p = parse("REC_2026_03_06_07_25_52_F.MP4").unwrap();
        assert_eq!(p.channel_label, LABEL_FRONT);
        assert_eq!(p.event_mode, EventMode::Normal);
        assert_eq!(
            p.start_time,
            NaiveDate::from_ymd_opt(2026, 3, 6)
                .unwrap()
                .and_hms_opt(7, 25, 52)
                .unwrap()
        );
        assert!(p.group_key.starts_with("tw:"));
    }

    #[test]
    fn parses_thinkware_rec_rear() {
        let p = parse("REC_2026_03_06_07_25_52_R.MP4").unwrap();
        assert_eq!(p.channel_label, LABEL_REAR);
    }

    #[test]
    fn thinkware_pair_shares_group_key() {
        let f = parse("REC_2026_03_06_07_25_52_F.MP4").unwrap();
        let r = parse("REC_2026_03_06_07_25_52_R.MP4").unwrap();
        assert_eq!(f.group_key, r.group_key);
    }

    #[test]
    fn parses_thinkware_event_prefix() {
        let p = parse("EVT_2026_03_06_07_25_52_F.MP4").unwrap();
        assert_eq!(p.event_mode, EventMode::Event);
    }

    #[test]
    fn parses_thinkware_manual_prefix() {
        let p = parse("MAN_2023_11_03_06_43_39_F.MP4").unwrap();
        assert_eq!(p.channel_label, LABEL_FRONT);
        assert_eq!(p.event_mode, EventMode::Normal);
        assert!(p.group_key.starts_with("tw:"));
    }

    #[test]
    fn parses_thinkware_unknown_prefix_defaults_to_normal() {
        // Parking and motion-timelapse prefixes aren't confirmed, but any
        // 3-letter uppercase prefix with the Thinkware shape should parse
        // (and not land in scan errors). Default event mode is Normal.
        let p = parse("PKG_2026_03_06_07_25_52_F.MP4").unwrap();
        assert_eq!(p.event_mode, EventMode::Normal);
        assert!(p.group_key.contains("_PKG"));
    }

    #[test]
    fn thinkware_rejects_non_uppercase_prefix() {
        // Guard rail: don't accept random 3-char prefixes that aren't
        // clearly Thinkware-style.
        assert!(parse("rec_2026_03_06_07_25_52_F.MP4").is_err());
        assert!(parse("12x_2026_03_06_07_25_52_F.MP4").is_err());
    }

    #[test]
    fn thinkware_does_not_collide_with_wolfbox() {
        // Both parsers have distinct shapes — no file should match both.
        assert!(WolfBoxParser
            .parse("REC_2026_03_06_07_25_52_F.MP4")
            .is_none());
        assert!(ThinkwareParser
            .parse("2026_03_15_173951_02_F.MP4")
            .is_none());
    }

    // ── Generic 4-channel ──────────────────────────────────────────────────

    #[test]
    fn parses_generic_4channel_letter() {
        let p = parse("2026_03_06_072552_A.MP4").unwrap();
        assert_eq!(p.channel_label, "Channel A");
        assert!(p.group_key.starts_with("g4:"));
    }

    #[test]
    fn parses_generic_4channel_digit() {
        let p = parse("2026_03_06_072552_3.MP4").unwrap();
        assert_eq!(p.channel_label, "Channel 3");
    }

    #[test]
    fn generic_4channel_shares_group_key() {
        let a = parse("2026_03_06_072552_A.MP4").unwrap();
        let b = parse("2026_03_06_072552_B.MP4").unwrap();
        let c = parse("2026_03_06_072552_C.MP4").unwrap();
        let d = parse("2026_03_06_072552_D.MP4").unwrap();
        assert_eq!(a.group_key, b.group_key);
        assert_eq!(b.group_key, c.group_key);
        assert_eq!(c.group_key, d.group_key);
    }

    // ── Rejection ──────────────────────────────────────────────────────────

    #[test]
    fn rejects_bad_extension() {
        assert!(parse("2026_03_23_094634_00_F.avi").is_err());
    }

    #[test]
    fn rejects_gibberish() {
        assert!(parse("hello.mp4").is_err());
        assert!(parse("VID_20200101.MP4").is_err());
    }

    #[test]
    fn rejects_bad_timestamp() {
        // Invalid month — Wolf Box parser rejects, generic parser also fails
        // because no timestamp substring parses.
        assert!(parse("2026_13_40_994634_00_F.MP4").is_err());
    }
}
