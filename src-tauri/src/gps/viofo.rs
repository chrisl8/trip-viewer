//! VIOFO / Novatek GPS decoder for A229-family MP4 files.
//!
//! Novatek MP4s used by VIOFO contain a `gps ` descriptor box inside `moov`.
//! After an 8-byte vendor header, the box contains `(offset, size)` pairs
//! pointing to top-level `free` boxes. A referenced `free` box starts with
//! the magic `GPS ` and contains one GPS record.
//!
//! This decoder deliberately preserves descriptor order and hands the raw
//! observations to the camera-neutral GPS consistency validator. Embedded UTC
//! timestamps are never used to reorder the source records: a malformed clock
//! field must not be allowed to move a damaged point to another part of a trip.

use crate::error::AppError;
mod validation;
use self::validation::{FieldConsistency, GpsObservation, ValidationContext};
use crate::model::GpsPoint;
use chrono::{NaiveDate, NaiveDateTime};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const KNOT_TO_MPS: f64 = 0.514_444;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    size: u64,
    kind: [u8; 4],
    body_start: u64,
}

impl BoxHeader {
    fn end(self, start: u64) -> Option<u64> {
        start.checked_add(self.size)
    }
}

#[derive(Debug, Clone, Copy)]
struct RawGpsRecord {
    utc: Option<NaiveDateTime>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    speed_mps: Option<f64>,
    heading_deg: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimedGpsPoint {
    pub utc: NaiveDateTime,
    pub point: GpsPoint,
}

/// Extract VIOFO points while retaining each surviving record's UTC timestamp.
///
/// `duration_s` is the measured duration of this exact MP4. When supplied, it
/// becomes a camera-neutral temporal constraint: two UTC observations from the
/// same clip cannot be farther apart than the clip itself. The bound is rounded
/// up to whole seconds because the Novatek GPS clock is second-resolution.
pub(crate) fn extract_timed(
    path: &Path,
    duration_s: Option<f64>,
) -> Result<Vec<TimedGpsPoint>, AppError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let descriptors = read_gps_descriptors(&mut file, file_len)?;
    if descriptors.is_empty() {
        eprintln!("viofo gps: no gps descriptor box in {}", path.display());
        return Ok(vec![]);
    }

    let mut decoded = Vec::with_capacity(descriptors.len());
    for (offset, size) in descriptors {
        if let Some(record) = read_gps_free_box(&mut file, file_len, offset, size)? {
            decoded.push(record);
        }
    }

    if decoded.is_empty() {
        eprintln!(
            "viofo gps: descriptor box found but no decodable GPS records in {}",
            path.display()
        );
        return Ok(vec![]);
    }

    let observations: Vec<GpsObservation> = decoded
        .iter()
        .enumerate()
        .map(|(source_order, record)| GpsObservation {
            source_order,
            time_ms: record.utc.map(|ts| ts.and_utc().timestamp_millis()),
            latitude: record.latitude,
            longitude: record.longitude,
            speed_mps: record.speed_mps,
            heading_deg: record.heading_deg,
        })
        .collect();

    let max_time_span_ms = duration_s
        .filter(|duration| duration.is_finite() && *duration >= 0.0)
        .map(|duration| (duration.ceil() * 1000.0) as i64);
    let validated = validation::validate(&observations, ValidationContext { max_time_span_ms });

    let temporal_contradictions = validated
        .iter()
        .filter(|item| item.time == FieldConsistency::Contradictory)
        .count();
    let temporal_unknown = validated
        .iter()
        .filter(|item| item.time == FieldConsistency::Unknown)
        .count();
    let spatial_exclusions = validated
        .iter()
        .filter(|item| {
            item.time == FieldConsistency::Consistent
                && (item.latitude != FieldConsistency::Consistent
                    || item.longitude != FieldConsistency::Consistent)
        })
        .count();

    if temporal_contradictions > 0 || temporal_unknown > 0 || spatial_exclusions > 0 {
        eprintln!(
            "viofo gps: validation in {}: {temporal_contradictions} temporal contradiction(s), {temporal_unknown} temporal ambiguous record(s), {spatial_exclusions} spatial exclusion(s)",
            path.display()
        );
    }

    let mut trusted = Vec::new();
    for (record, validation) in decoded.into_iter().zip(validated.into_iter()) {
        if !validation.usable_timed_position() {
            continue;
        }

        let Some(utc) = record.utc else {
            continue;
        };
        let (Some(lat), Some(lon)) = (record.latitude, record.longitude) else {
            continue;
        };

        trusted.push(TimedGpsPoint {
            utc,
            point: GpsPoint {
                t_offset_s: 0.0,
                lat,
                lon,
                speed_mps: if validation.speed == FieldConsistency::Consistent {
                    record.speed_mps.unwrap_or(0.0)
                } else {
                    0.0
                },
                heading_deg: if validation.heading == FieldConsistency::Consistent {
                    record.heading_deg.unwrap_or(0.0)
                } else {
                    0.0
                },
                altitude_m: 0.0,
                fix_ok: true,
            },
        });
    }

    if trusted.is_empty() {
        return Ok(vec![]);
    }

    // The validator guarantees that records used as timed positions retain a
    // monotonic clock. Keep descriptor order and establish a relative axis from
    // the first trusted observation; the separate timing module later places
    // that first fix on the video timeline.
    let first_ts = trusted[0].utc;
    for item in &mut trusted {
        item.point.t_offset_s = (item.utc - first_ts).num_milliseconds() as f64 / 1000.0;
    }

    Ok(trusted)
}

/// Decoder-only convenience entry point used by focused unit tests and future
/// callers that do not have a measured clip duration available.
pub fn extract(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    Ok(extract_timed(path, None)?
        .into_iter()
        .map(|item| item.point)
        .collect())
}

fn read_gps_descriptors(file: &mut File, file_len: u64) -> Result<Vec<(u64, u64)>, AppError> {
    let mut top = 0u64;
    while top + 8 <= file_len {
        let Some(header) = read_box_header(file, top, file_len)? else {
            break;
        };
        let Some(end) = header.end(top) else {
            return Err(AppError::Parse("MP4 box size overflow".into()));
        };
        if end > file_len || header.size < header.body_start.saturating_sub(top) {
            break;
        }

        if &header.kind == b"moov" {
            if let Some(found) = find_gps_box(file, header.body_start, end, file_len)? {
                return Ok(found);
            }
        }
        if header.size == 0 {
            break;
        }
        top = end;
    }
    Ok(vec![])
}

fn find_gps_box(
    file: &mut File,
    mut pos: u64,
    moov_end: u64,
    file_len: u64,
) -> Result<Option<Vec<(u64, u64)>>, AppError> {
    while pos + 8 <= moov_end {
        let Some(header) = read_box_header(file, pos, file_len)? else {
            break;
        };
        let Some(end) = header.end(pos) else {
            return Err(AppError::Parse("MP4 child box size overflow".into()));
        };
        if end > moov_end || header.size == 0 {
            break;
        }

        if &header.kind == b"gps " {
            // Novatek stores an 8-byte vendor header before the descriptor
            // pairs. This corresponds to the established parser convention of
            // starting 16 bytes after the `gps ` box start (8 box + 8 vendor).
            let mut cursor = header.body_start.saturating_add(8);
            let mut out = Vec::new();
            if cursor > end {
                return Ok(Some(out));
            }
            file.seek(SeekFrom::Start(cursor))?;
            // Descriptor pairs are contiguous. Read through a bounded buffer
            // instead of seeking and reading eight bytes for every pair.
            // Take prevents reading beyond this box; descriptor order is kept.
            let mut pairs = BufReader::with_capacity(16 * 1024, file.take(end - cursor));
            while cursor + 8 <= end {
                let mut pair = [0u8; 8];
                pairs.read_exact(&mut pair)?;
                let offset = u32::from_be_bytes(pair[0..4].try_into().unwrap()) as u64;
                let size = u32::from_be_bytes(pair[4..8].try_into().unwrap()) as u64;
                if offset != 0 && size >= 12 && offset.saturating_add(size) <= file_len {
                    out.push((offset, size));
                }
                cursor += 8;
            }
            return Ok(Some(out));
        }

        pos = end;
    }
    Ok(None)
}

fn read_gps_free_box(
    file: &mut File,
    file_len: u64,
    offset: u64,
    descriptor_size: u64,
) -> Result<Option<RawGpsRecord>, AppError> {
    if offset.saturating_add(descriptor_size) > file_len || descriptor_size < 12 {
        return Ok(None);
    }

    file.seek(SeekFrom::Start(offset))?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head)?;
    let box_size = u32::from_be_bytes(head[0..4].try_into().unwrap()) as u64;
    if box_size != descriptor_size || &head[4..8] != b"free" || &head[8..12] != b"GPS " {
        return Ok(None);
    }

    let payload_len = descriptor_size.saturating_sub(12) as usize;
    let mut payload = vec![0u8; payload_len];
    file.read_exact(&mut payload)?;
    Ok(decode_packet(&payload))
}

fn decode_packet(data: &[u8]) -> Option<RawGpsRecord> {
    let offset = find_record_offset(data)?;
    if offset + 44 > data.len() {
        return None;
    }

    let hour = read_u32_le(data, offset)?;
    let minute = read_u32_le(data, offset + 4)?;
    let second = read_u32_le(data, offset + 8)?;
    let year = read_u32_le(data, offset + 12)?;
    let month = read_u32_le(data, offset + 16)?;
    let day = read_u32_le(data, offset + 20)?;

    let lat_hemi = data[offset + 25];
    let lon_hemi = data[offset + 26];
    let lat_raw = read_f32_le(data, offset + 28)? as f64;
    let lon_raw = read_f32_le(data, offset + 32)? as f64;
    let speed_knots = read_f32_le(data, offset + 36)? as f64;
    let bearing = read_f32_le(data, offset + 40)? as f64;

    let utc = i32::try_from(year).ok().and_then(|year| {
        NaiveDate::from_ymd_opt(2000 + year, month, day)
            .and_then(|date| date.and_hms_opt(hour, minute, second))
    });

    // Preserve malformed scalar fields as NaN/negative values so the generic
    // validator can mark the individual field contradictory instead of the
    // decoder silently discarding the entire record.
    let latitude = Some(nmea_coord_to_degrees(lat_raw, lat_hemi).unwrap_or(f64::NAN));
    let longitude = Some(nmea_coord_to_degrees(lon_raw, lon_hemi).unwrap_or(f64::NAN));
    let speed_mps = Some(speed_knots * KNOT_TO_MPS);
    let heading_deg = Some(if bearing.is_finite() {
        bearing.rem_euclid(360.0)
    } else {
        bearing
    });

    Some(RawGpsRecord {
        utc,
        latitude,
        longitude,
        speed_mps,
        heading_deg,
    })
}

fn find_record_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 27 {
        return None;
    }
    // Find the `A N/S E/W` fix marker. The marker is 24 bytes after the
    // beginning of the canonical Novatek record.
    for marker in (24..=data.len().saturating_sub(3)).rev() {
        let active = data[marker];
        let lat_hemi = data[marker + 1];
        let lon_hemi = data[marker + 2];
        if active == b'A' && matches!(lat_hemi, b'N' | b'S') && matches!(lon_hemi, b'E' | b'W') {
            return marker.checked_sub(24);
        }
    }
    None
}

fn nmea_coord_to_degrees(raw: f64, hemi: u8) -> Option<f64> {
    if !raw.is_finite() || raw < 0.0 {
        return None;
    }
    let degrees = (raw / 100.0).floor();
    let minutes = raw - degrees * 100.0;
    if !(0.0..60.0).contains(&minutes) {
        return None;
    }
    let mut value = degrees + minutes / 60.0;
    match hemi {
        b'N' | b'E' => {}
        b'S' | b'W' => value = -value,
        _ => return None,
    }
    Some(value)
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?))
}

fn read_f32_le(data: &[u8], off: usize) -> Option<f32> {
    Some(f32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?))
}

fn read_box_header(
    file: &mut File,
    start: u64,
    file_len: u64,
) -> Result<Option<BoxHeader>, AppError> {
    if start + 8 > file_len {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(start))?;
    let mut hdr = [0u8; 8];
    file.read_exact(&mut hdr)?;
    let size32 = u32::from_be_bytes(hdr[0..4].try_into().unwrap());
    let kind = hdr[4..8].try_into().unwrap();

    let (size, body_start) = match size32 {
        0 => (file_len - start, start + 8),
        1 => {
            if start + 16 > file_len {
                return Ok(None);
            }
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext)?;
            (u64::from_be_bytes(ext), start + 16)
        }
        n => (n as u64, start + 8),
    };
    if size < body_start - start {
        return Ok(None);
    }
    Ok(Some(BoxHeader {
        size,
        kind,
        body_start,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_packet() -> Vec<u8> {
        let mut p = vec![0u8; 48];
        p[0..4].copy_from_slice(&6u32.to_le_bytes());
        p[4..8].copy_from_slice(&52u32.to_le_bytes());
        p[8..12].copy_from_slice(&29u32.to_le_bytes());
        p[12..16].copy_from_slice(&26u32.to_le_bytes());
        p[16..20].copy_from_slice(&8u32.to_le_bytes());
        p[20..24].copy_from_slice(&31u32.to_le_bytes());
        p[24] = b'A';
        p[25] = b'N';
        p[26] = b'E';
        p[27] = 0;
        p[28..32].copy_from_slice(&(5332.5f32).to_le_bytes());
        p[32..36].copy_from_slice(&(1001.5f32).to_le_bytes());
        p[36..40].copy_from_slice(&(10.0f32).to_le_bytes());
        p[40..44].copy_from_slice(&(123.0f32).to_le_bytes());
        p
    }

    #[test]
    fn decodes_novatek_packet() {
        let record = decode_packet(&sample_packet()).unwrap();
        assert_eq!(
            record.utc.unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 31)
                .unwrap()
                .and_hms_opt(6, 52, 29)
                .unwrap()
        );
        assert!((record.latitude.unwrap() - 53.5416667).abs() < 0.00001);
        assert!((record.longitude.unwrap() - 10.025).abs() < 0.00001);
        assert!((record.speed_mps.unwrap() - 5.14444).abs() < 0.0001);
        assert_eq!(record.heading_deg.unwrap(), 123.0);
    }

    #[test]
    fn converts_south_and_west() {
        assert!((nmea_coord_to_degrees(5332.5, b'S').unwrap() + 53.5416667).abs() < 0.00001);
        assert!((nmea_coord_to_degrees(1001.5, b'W').unwrap() + 10.025).abs() < 0.00001);
    }

    #[test]
    fn short_and_empty_descriptor_boxes_are_safe_without_unsigned_underflow() {
        for gps_size in [8u32, 12, 16, 20] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&8u32.to_be_bytes());
            bytes.extend_from_slice(b"ftyp");
            bytes.extend_from_slice(&(8 + gps_size).to_be_bytes());
            bytes.extend_from_slice(b"moov");
            bytes.extend_from_slice(&gps_size.to_be_bytes());
            bytes.extend_from_slice(b"gps ");
            bytes.resize(bytes.len() + gps_size as usize - 8, 0);
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("short.MP4");
            std::fs::write(&path, bytes).unwrap();
            assert!(extract(&path).unwrap().is_empty());
        }
    }

    #[test]
    fn extracts_from_minimal_novatek_mp4_layout() {
        let packet = sample_packet();
        let free_size = 12 + packet.len() as u32;

        // ftyp (8) + moov (32) => referenced free box starts at byte 40.
        let free_offset = 40u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(b"ftyp");

        bytes.extend_from_slice(&32u32.to_be_bytes());
        bytes.extend_from_slice(b"moov");
        bytes.extend_from_slice(&24u32.to_be_bytes());
        bytes.extend_from_slice(b"gps ");
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&free_offset.to_be_bytes());
        bytes.extend_from_slice(&free_size.to_be_bytes());

        bytes.extend_from_slice(&free_size.to_be_bytes());
        bytes.extend_from_slice(b"free");
        bytes.extend_from_slice(b"GPS ");
        bytes.extend_from_slice(&packet);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026_0831_065229_000646F.MP4");
        let mut f = File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
        drop(f);

        let points = extract(&path).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].t_offset_s, 0.0);
        assert!(points[0].fix_ok);
    }
}

/// Decode a real clip with the accepted duration-bound validator and timing.
pub(crate) fn extract_video_timed(path: &Path) -> Result<Vec<TimedGpsPoint>, AppError> {
    let duration_s = crate::metadata::mp4_probe::probe(path)?.duration_s;
    let mut timed = extract_timed(path, Some(duration_s))?;
    if let Some(first) = timed.first() {
        if let Some(delay) = super::viofo_timing::first_fix_delay_s(path, first.utc, duration_s) {
            for item in &mut timed {
                item.point.t_offset_s += delay;
            }
        }
    }
    Ok(timed)
}

pub(crate) fn extract_video(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    Ok(extract_video_timed(path)?
        .into_iter()
        .map(|p| p.point)
        .collect())
}

pub(crate) fn validate_boundary(
    left_path: &str,
    left: &mut Vec<TimedGpsPoint>,
    right_path: &str,
    right: &mut Vec<TimedGpsPoint>,
) {
    if left.len() < 2 || right.len() < 2 || !boundary_has_local_utc_continuity(left, right) {
        return;
    }

    let left_last = left.len() - 1;
    let points = [&left[left_last - 1], &left[left_last], &right[0], &right[1]];
    let observations: Vec<validation::GpsObservation> = points
        .iter()
        .enumerate()
        .map(|(source_order, item)| boundary_observation(item, source_order))
        .collect();
    let validated = validation::validate(&observations, validation::ValidationContext::default());

    let drop_left = !validated[1].usable_timed_position();
    let drop_right = !validated[2].usable_timed_position();

    if drop_left {
        left.pop();
        eprintln!(
            "viofo gps: boundary validation excluded terminal route point in {left_path} using UTC evidence from {right_path}"
        );
    }
    if drop_right {
        right.remove(0);
        eprintln!(
            "viofo gps: boundary validation excluded initial route point in {right_path} using UTC evidence from {left_path}"
        );
    }
}

fn boundary_observation(item: &TimedGpsPoint, source_order: usize) -> validation::GpsObservation {
    // A zero speed is valid data, but after the first validation pass it can
    // also be the neutral UI fallback for an unusable speed/heading field. Do
    // not use such a value as an independent cross-file witness. This only
    // weakens the boundary proof; it never rejects a route point by itself.
    let moving = item.point.speed_mps > 0.0;
    validation::GpsObservation {
        source_order,
        time_ms: Some(item.utc.and_utc().timestamp_millis()),
        latitude: Some(item.point.lat),
        longitude: Some(item.point.lon),
        speed_mps: moving.then_some(item.point.speed_mps),
        heading_deg: moving.then_some(item.point.heading_deg),
    }
}

fn boundary_has_local_utc_continuity(left: &[TimedGpsPoint], right: &[TimedGpsPoint]) -> bool {
    let left_times: Vec<i64> = left
        .iter()
        .map(|item| item.utc.and_utc().timestamp_millis())
        .collect();
    let right_times: Vec<i64> = right
        .iter()
        .map(|item| item.utc.and_utc().timestamp_millis())
        .collect();
    boundary_times_are_locally_continuous(&left_times, &right_times)
}

fn boundary_times_are_locally_continuous(left: &[i64], right: &[i64]) -> bool {
    if left.len() < 2 || right.len() < 2 {
        return false;
    }

    let cross_gap = right[0] - left[left.len() - 1];
    if cross_gap < 0 {
        return false;
    }

    let left_gap = left
        .windows(2)
        .rev()
        .find_map(|pair| positive_time_gap(pair[0], pair[1]));
    let right_gap = right
        .windows(2)
        .find_map(|pair| positive_time_gap(pair[0], pair[1]));

    match (left_gap, right_gap) {
        (Some(left_gap), Some(right_gap)) => cross_gap <= left_gap.max(right_gap),
        _ => false,
    }
}

fn positive_time_gap(a: i64, b: i64) -> Option<i64> {
    b.checked_sub(a).filter(|gap| *gap > 0)
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    fn timed(ms: i64, lon: f64, speed_mps: f64, heading_deg: f64) -> TimedGpsPoint {
        let utc = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
            .expect("valid test timestamp")
            .naive_utc();
        TimedGpsPoint {
            utc,
            point: GpsPoint {
                t_offset_s: ms as f64 / 1000.0,
                lat: 0.0,
                lon,
                speed_mps,
                heading_deg,
                altitude_m: 0.0,
                fix_ok: true,
            },
        }
    }

    #[test]
    fn boundary_cadence_is_derived_from_observed_utc() {
        assert!(boundary_times_are_locally_continuous(
            &[0, 2_000, 4_000],
            &[6_000, 8_000, 10_000]
        ));
        assert!(boundary_times_are_locally_continuous(
            &[0, 1_000, 1_000],
            &[2_000, 2_000, 3_000]
        ));
        assert!(!boundary_times_are_locally_continuous(
            &[0, 1_000, 2_000],
            &[60_000, 61_000]
        ));
    }

    #[test]
    fn boundary_context_rejects_corrupt_terminal_position() {
        let mut left = vec![
            timed(0, 0.0, 11.12, 90.0),
            timed(1_000, 0.0001, 11.12, 90.0),
            timed(2_000, 20.0, 0.0, 0.0),
        ];
        let mut right = vec![
            timed(3_000, 0.0003, 11.12, 90.0),
            timed(4_000, 0.0004, 11.12, 90.0),
        ];

        validate_boundary("left", &mut left, "right", &mut right);

        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        assert_eq!(left.last().unwrap().point.lon, 0.0001);
    }

    #[test]
    fn boundary_context_does_not_bridge_a_real_utc_pause() {
        let mut left = vec![
            timed(0, 0.0, 11.12, 90.0),
            timed(1_000, 0.0001, 11.12, 90.0),
            timed(2_000, 20.0, 0.0, 0.0),
        ];
        let mut right = vec![
            timed(60_000, 0.0003, 11.12, 90.0),
            timed(61_000, 0.0004, 11.12, 90.0),
        ];

        validate_boundary("left", &mut left, "right", &mut right);

        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 2);
    }
    #[test]
    fn boundary_context_rejects_corrupt_initial_position() {
        let mut left = vec![timed(0, 0.0, 11.12, 90.0), timed(1000, 0.0001, 11.12, 90.0)];
        let mut right = vec![
            timed(2000, 20.0, 0.0, 0.0),
            timed(3000, 0.0003, 11.12, 90.0),
            timed(4000, 0.0004, 11.12, 90.0),
        ];
        validate_boundary("left", &mut left, "right", &mut right);
        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        assert_eq!(right[0].point.lon, 0.0003);
    }

    #[test]
    fn boundary_without_neighbor_evidence_does_not_guess() {
        let mut left = vec![timed(0, 0.0, 11.12, 90.0), timed(1000, 20.0, 0.0, 0.0)];
        validate_boundary("left", &mut left, "empty", &mut vec![]);
        assert_eq!(left.len(), 2);
    }
}
