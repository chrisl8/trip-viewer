use crate::error::AppError;
use crate::model::ChannelKind;
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
    pub channel: ChannelKind,
    pub base_key: String,
}

pub fn parse(filename: &str) -> Result<ParsedName, AppError> {
    let stem = filename
        .strip_suffix(".MP4")
        .or_else(|| filename.strip_suffix(".mp4"))
        .ok_or_else(|| AppError::InvalidFilename(filename.into()))?;

    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() != 6 {
        return Err(AppError::InvalidFilename(filename.into()));
    }

    let (year, month, day, hms, event_code, chan) =
        (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]);

    let dt_str = format!("{year}_{month}_{day}_{hms}");
    let start_time = NaiveDateTime::parse_from_str(&dt_str, "%Y_%m_%d_%H%M%S")
        .map_err(|e| AppError::Parse(format!("timestamp in {filename}: {e}")))?;

    let event_mode = match event_code {
        "00" => EventMode::Normal,
        "02" => EventMode::Event,
        other => {
            let n: u8 = other
                .parse()
                .map_err(|_| AppError::InvalidFilename(filename.into()))?;
            EventMode::Other(n)
        }
    };

    let channel = match chan {
        "F" => ChannelKind::Front,
        "I" => ChannelKind::Interior,
        "R" => ChannelKind::Rear,
        _ => return Err(AppError::InvalidFilename(filename.into())),
    };

    let base_key = format!("{year}_{month}_{day}_{hms}_{event_code}");

    Ok(ParsedName {
        start_time,
        event_mode,
        channel,
        base_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parses_normal_front() {
        let p = parse("2026_03_23_094634_00_F.MP4").unwrap();
        assert_eq!(p.channel, ChannelKind::Front);
        assert_eq!(p.event_mode, EventMode::Normal);
        assert_eq!(
            p.start_time,
            NaiveDate::from_ymd_opt(2026, 3, 23)
                .unwrap()
                .and_hms_opt(9, 46, 34)
                .unwrap()
        );
        assert_eq!(p.base_key, "2026_03_23_094634_00");
    }

    #[test]
    fn parses_event_interior() {
        let p = parse("2026_03_15_173951_02_I.MP4").unwrap();
        assert_eq!(p.channel, ChannelKind::Interior);
        assert_eq!(p.event_mode, EventMode::Event);
        assert_eq!(p.base_key, "2026_03_15_173951_02");
    }

    #[test]
    fn parses_rear_lowercase_extension() {
        let p = parse("2026_04_10_162529_00_R.mp4").unwrap();
        assert_eq!(p.channel, ChannelKind::Rear);
    }

    #[test]
    fn triplet_shares_base_key() {
        let f = parse("2026_03_15_173951_02_F.MP4").unwrap();
        let i = parse("2026_03_15_173951_02_I.MP4").unwrap();
        let r = parse("2026_03_15_173951_02_R.MP4").unwrap();
        assert_eq!(f.base_key, i.base_key);
        assert_eq!(i.base_key, r.base_key);
    }

    #[test]
    fn rejects_bad_extension() {
        assert!(parse("2026_03_23_094634_00_F.avi").is_err());
    }

    #[test]
    fn rejects_wrong_part_count() {
        assert!(parse("2026_03_23_094634_F.MP4").is_err());
        assert!(parse("2026_03_23_094634_00_F_extra.MP4").is_err());
    }

    #[test]
    fn rejects_invalid_channel_letter() {
        assert!(parse("2026_03_23_094634_00_X.MP4").is_err());
    }

    #[test]
    fn rejects_bad_timestamp() {
        assert!(parse("2026_13_40_994634_00_F.MP4").is_err());
    }

    #[test]
    fn accepts_other_event_code() {
        let p = parse("2026_03_23_094634_05_F.MP4").unwrap();
        assert_eq!(p.event_mode, EventMode::Other(5));
    }
}
