//! Wolf Box ShenShu MetaData GPS decoder.
//!
//! The Wolf Box firmware embeds GPS in a custom binary "meta" track. The
//! layout below was reverse-engineered — there's no upstream spec. Each MP4
//! sample is 1000 bytes and contains one GPS fix at 1 Hz (the track claims
//! 5 Hz via timescale/duration, but the GPS H:M:S field advances by one
//! second per sample, so we trust that rather than the track-level timing).

use crate::error::AppError;
use crate::model::GpsPoint;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

// ShenShu MetaData binary struct offsets (all LE i32 at 4-byte alignment).
const OFF_STATUS: usize = 0x00;
const OFF_LAT: usize = 0x28;
const OFF_LAT_SCALE: usize = 0x30;
const OFF_LON: usize = 0x38;
const OFF_LON_SCALE: usize = 0x40;
const OFF_SPEED: usize = 0x48;
const OFF_ALT: usize = 0x58;

const MIN_SAMPLE_LEN: usize = 0x78;

pub fn extract(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let reader = BufReader::new(file);

    let mut mp4 = match mp4::Mp4Reader::read_header(reader, size) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gps: mp4 header failed for {}: {e}", path.display());
            return Ok(vec![]);
        }
    };

    let meta_track_id = match find_metadata_track(&mp4) {
        Some(id) => id,
        None => return Ok(vec![]),
    };

    let sample_count = mp4.tracks()[&meta_track_id].sample_count();
    let mut points: Vec<GpsPoint> = Vec::new();

    for sample_id in 1..=sample_count {
        let sample = match mp4.read_sample(meta_track_id, sample_id) {
            Ok(Some(s)) => s,
            _ => continue,
        };

        let d = &sample.bytes[..];
        if d.len() < MIN_SAMPLE_LEN {
            continue;
        }

        let status = le_i32(d, OFF_STATUS);
        if status != 1 {
            continue;
        }

        let lat_raw = le_i32(d, OFF_LAT);
        let lat_scale = le_i32(d, OFF_LAT_SCALE);
        let lon_raw = le_i32(d, OFF_LON);
        let lon_scale = le_i32(d, OFF_LON_SCALE);
        let lat = nmea_to_deg(lat_raw, lat_scale);
        let lon = nmea_to_deg(lon_raw, lon_scale);

        if lat.abs() < 0.001 && lon.abs() < 0.001 {
            continue;
        }

        // GPS speed is in 0.01 knots (NMEA standard); 1 knot = 0.514444 m/s
        let speed_knots = le_i32(d, OFF_SPEED) as f64 / 100.0;
        let altitude_m = le_i32(d, OFF_ALT) as f64 / 100.0;

        // 1 sample = 1 second (despite MP4 track claiming 5Hz)
        let t_offset_s = (sample_id - 1) as f64;

        points.push(GpsPoint {
            t_offset_s,
            lat,
            lon,
            speed_mps: speed_knots * 0.514444,
            heading_deg: 0.0,
            altitude_m,
            fix_ok: true,
        });
    }

    // Compute heading from consecutive positions
    for i in 0..points.len().saturating_sub(1) {
        let bearing = bearing_deg(points[i].lat, points[i].lon, points[i + 1].lat, points[i + 1].lon);
        points[i].heading_deg = bearing;
    }
    if let Some(last_idx) = points.len().checked_sub(1) {
        if last_idx > 0 {
            points[last_idx].heading_deg = points[last_idx - 1].heading_deg;
        }
    }

    Ok(points)
}

fn find_metadata_track(mp4: &mp4::Mp4Reader<BufReader<File>>) -> Option<u32> {
    for (id, track) in mp4.tracks() {
        if track.trak.mdia.hdlr.handler_type.value == *b"meta" {
            return Some(*id);
        }
    }
    None
}

fn nmea_to_deg(raw: i32, scale: i32) -> f64 {
    let scale = if scale > 0 { scale } else { 100000 };
    let nmea = raw as f64 / scale as f64;
    let sign = nmea.signum();
    let nmea = nmea.abs();
    let degrees = (nmea / 100.0).trunc();
    let minutes = nmea - degrees * 100.0;
    sign * (degrees + minutes / 60.0)
}

fn le_i32(d: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (lat1, lon1) = (lat1.to_radians(), lon1.to_radians());
    let (lat2, lon2) = (lat2.to_radians(), lon2.to_radians());
    let dlon = lon2 - lon1;
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}
