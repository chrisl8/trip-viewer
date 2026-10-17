//! NovaTek-family GPS decoder based on the algorithm described in
//! https://sergei.nz/extracting-gps-data-from-viofo-a119-and-other-novatek-powered-cameras
//! Tested with Viofo A129 Duo
//!
//! In very simple terms the MP4 container consists of atoms/boxes (the name
//! depends which documentation you read). The boxes can contain other boxes.
//! 
//! Each box starts with 8 byte “header” (including the beginning of the
//! file). The first 4 bytes is the size of the box (big endian unsigned int),
//! the second 4 bytes contains 4 character string name/type of the box. The
//! size includes itself (so valid size is >= 0x0008 unless the special type
//! of large box which I will conveniently omit in this post ;)). Basically
//! MP4 container can be treated as some rudimentary file system.
//! 
//! Novatek special “gps ” box
//! The “gps ” box is found inside of “moov” box.
//! The “gps ” box stores the file offset (in bytes) and size (in bytes) for
//! each GPS data box.  The first 8 bytes in the “gps ” box contain version
//! and encoded build date. I simply chose to ignore this data.
//! The subsequent 8 bytes contain 4 byte file offset (absolute) and 4 byte
//! size (offset and size are big-endian unsigned ints).
//! 
//! Novatek special “free” box beginning “GPS “
//! Not to be confused with “gps ” box ;).
//! This box sits in “free” box in midst of data chunks. The box can be
//! identified with a magic “GPS ” string.  For some reason Novatek decided to
//! store all the data in this box in little-endian format…

use crate::error::AppError;
use crate::model::GpsPoint;
use chrono::{NaiveDate, NaiveDateTime};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use num_traits::cast::FromPrimitive;

const GPS_HEADER_SIZE: usize = 12;
const GPS_RECORD_SIZE: usize = 44;
const OFF_DATE: usize = 0; // YY, MM, DD, HH, MM, SS (24 bytes)
const OFF_LAT_HEMI: usize = 25;
const OFF_LON_HEMI: usize = 26;
const OFF_LAT_F64: usize = 28;
const OFF_LON_F64: usize = 32;
const OFF_SPEED_KNOTS: usize = 36;
const OFF_BEARING: usize = 40;

fn in_range_lat(v: f64) -> bool {
    v.is_finite() && v.abs() <= 90.0
}
fn in_range_lon(v: f64) -> bool {
    v.is_finite() && v.abs() <= 180.0
}

pub fn extract(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    let body = match read_gps_atoms(path)? {
        Some(b) => b,
        None => {
            return Ok(vec![]);
        }
    };
    if body.len() < GPS_RECORD_SIZE {
        eprintln!(
            "novatek gps: gps body too short ({}B) in {}",
            body.len(),
            path.display()
        );
        return Ok(vec![]);
    }

    let record_count = body.len() / GPS_RECORD_SIZE;
    let mut points: Vec<GpsPoint> = Vec::with_capacity(record_count);
    let mut first_ts: Option<NaiveDateTime> = None;

    for idx in 0..record_count {
        let rec = &body[idx * GPS_RECORD_SIZE..(idx + 1) * GPS_RECORD_SIZE];

        let lat_raw = f32::from_le_bytes(rec[OFF_LAT_F64..OFF_LAT_F64 + 4].try_into().unwrap()) as f64;
        let lon_raw = f32::from_le_bytes(rec[OFF_LON_F64..OFF_LON_F64 + 4].try_into().unwrap()) as f64;
        let lat = fix_coordinates(rec[OFF_LAT_HEMI], lat_raw);
        let lon = fix_coordinates(rec[OFF_LON_HEMI], lon_raw);
        let fix_ok = in_range_lat(lat) && in_range_lon(lon);

        let speed_knots = f32::from_le_bytes(rec[OFF_SPEED_KNOTS..OFF_SPEED_KNOTS + 4].try_into().unwrap()) as f64;
	let speed_mps = fix_speed(speed_knots);
	let bearing = f32::from_le_bytes(rec[OFF_BEARING..OFF_BEARING + 4].try_into().unwrap()) as f64;

        let ts = decode_timestamp(rec);
        let t_offset_s = match (first_ts, ts) {
            (None, Some(t)) => {
                first_ts = Some(t);
                0.0
            }
            (Some(t0), Some(t)) => (t - t0).num_milliseconds() as f64 / 1000.0,
            // No timestamp → fall back to record index (1 Hz assumption).
            _ => idx as f64,
        };

        points.push(GpsPoint {
            t_offset_s,
            lat: if fix_ok { lat } else { 0.0 },
            lon: if fix_ok { lon } else { 0.0 },
            speed_mps: speed_mps,
            heading_deg: bearing,
            altitude_m: 0.0,
            fix_ok,
        });
    }

    Ok(points)
}

fn decode_timestamp(rec: &[u8]) -> Option<NaiveDateTime> {
    let hh = u32::from_le_bytes(rec[OFF_DATE..OFF_DATE+4].try_into().unwrap());
    let mi = u32::from_le_bytes(rec[OFF_DATE+4..OFF_DATE+8].try_into().unwrap());
    let se = u32::from_le_bytes(rec[OFF_DATE+8..OFF_DATE+12].try_into().unwrap());
    let yy = i32::from_le_bytes(rec[OFF_DATE+12..OFF_DATE+16].try_into().unwrap());
    let mo = u32::from_le_bytes(rec[OFF_DATE+16..OFF_DATE+20].try_into().unwrap());
    let da = u32::from_le_bytes(rec[OFF_DATE+20..OFF_DATE+24].try_into().unwrap());
    NaiveDate::from_ymd_opt(2000 + yy, mo, da)?.and_hms_opt(hh, mi, se)
}

fn fix_coordinates(hemisphere: u8, coordinate: f64) -> f64 {
    let mut value = coordinate;

    // Convert DDDmm.mmmm to decimal degrees unless deobfuscate is true
    let minutes = value % 100.0;
    let degrees = value - minutes;
    value = (degrees / 100.0) + (minutes / 60.0);

    // Apply sign based on hemisphere
    if hemisphere == b'S' || hemisphere == b'W' {
        return -value;
    }

    value
}

fn fix_speed(speed : f64) -> f64 {
    // simple knot to m/s conversion; 1 knot = 0.514444 m/s """
    speed * 0.514444
}

/// Walk top-level atoms and return the useful part of the body bytes of `gps ` atoms if present.
/// `gps ` atoms we're looking for must be located within another atom named `moov`.
fn read_gps_atoms(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    let mut f = File::open(path)?;
    let file_len: u64 = f.metadata()?.len();
    let mut pos: u64 = 0;
    let mut moov_end: u64 = 0;
    let mut body = Vec::<u8>::new();

    body.reserve(2048);

    while pos < file_len {
        f.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if f.read(&mut hdr)? < 8 {
            break;
        }

        let size32 = u32::from_be_bytes(hdr[0..4].try_into().unwrap());
	if size32 < 8 {
	    break;
	}
	let container_limit: u64 = if moov_end != 0 { moov_end } else { file_len };
	if u64::from_u32(size32) > Some(container_limit.saturating_sub(pos)) {
            eprintln!("atom size exceeds container boundary {}", path.display());
	    break;
	}

        let atom: [u8; 4] = hdr[4..8].try_into().unwrap();
        let (atom_size, body_start) = (size32 as u64, pos + 8);

        if moov_end != 0 {
	    if &atom == b"gps " {
		// in addition to atom size and name (8 bytes), skip another header of 8 bytes
		f.seek(SeekFrom::Start(body_start+8))?;
		let sections_len: usize = atom_size.saturating_sub(16).try_into().unwrap();
		if sections_len == 0 || sections_len % 8 != 0 {
		    eprintln!("unexpected 'gps ' sections_len ({}) in {}", sections_len, path.display());
		    continue;
		}
		let mut sections = vec![0u8; sections_len];
		f.read_exact(&mut sections)?;

		// Handle the {offset, size} array to freeGPS sections
		let mut gps_records: usize = 0;
		for chunk in sections.chunks_exact(8) {
		    let data_pos = u32::from_be_bytes(chunk[0..4].try_into().unwrap());
		    let data_size: usize = u32::from_be_bytes(chunk[4..8].try_into().unwrap()) as usize;
		    if data_size <= GPS_HEADER_SIZE {
			eprintln!("skip invalid section length ({}) in {}", data_size, path.display());
			continue;
		    }

		    // Read section header and validate it
		    let mut sect = vec![0u8; data_size];
		    f.seek(SeekFrom::Start(data_pos as u64))?;
		    f.read_exact(&mut sect[0..GPS_HEADER_SIZE])?;
		    let sect_size: usize = u32::from_be_bytes(sect[0..4].try_into().unwrap()) as usize;
		    if sect_size != data_size {
			eprintln!("skip invalid section, length not matching ({} != {}) in {}", data_size, sect_size, path.display());
			continue;
		    }
		    if &sect[4..GPS_HEADER_SIZE] != b"freeGPS " {
			eprintln!("skip invalid section, magic not found ({:?} != {:?}) in {}", &sect[4..GPS_HEADER_SIZE], b"freeGPS ", path.display());
		    }

		    // Search for GPS points in the section data
		    f.read_exact(&mut sect[GPS_HEADER_SIZE..])?;
		    let ptr = get_gps_offset(&sect);

		    // Add GPS data to result vector
		    if ptr > GPS_HEADER_SIZE {
			gps_records += 1;
			body.extend_from_slice(&sect[ptr..ptr + GPS_RECORD_SIZE]);
		    }
		    /*let body_len: usize = body.len();
		    let append_len: usize = sect_size.saturating_sub(GPS_HEADER_SIZE).try_into().unwrap();
		    body.reserve(append_len.try_into().unwrap());
		    unsafe {
			body.set_len(body_len + append_len);
		    }
		    f.read_exact(&mut body[body_len..])?;
		    eprintln!("add {} bytes @{} to body {}", append_len, body_len, path.display());*/
		}
		if gps_records == 0 {
		    eprintln!("no GPS records found in {}", path.display());
		}
	    }
	} else {
	    if &atom == b"moov" {
		moov_end = pos.checked_add(atom_size).ok_or_else(|| {
		    AppError::Internal(format!("atom size overflow in {}", path.display()))
		})?;
		if moov_end <= file_len {
		    pos = body_start;
		    continue;
		} else {
		    eprintln!("'moov' size exceed EOF in {}", path.display());
		    moov_end = 0;
		} 
	    }
        }

        pos = pos.checked_add(atom_size).ok_or_else(|| {
            AppError::Internal(format!("atom size overflow in {}", path.display()))
        })?;
    }
    if body.len() == 0 {
            eprintln!("novatek gps: no gps atom in {}", path.display());
    }
    Ok(Some(body))
}

fn get_gps_offset(data: &Vec<u8>) -> usize {
    // Start at the end with 20 bytes allowed for trailing data
    // Use saturating_sub to prevent underflow if data is small (though checked above)
    let mut pointer = data.len().saturating_sub(20);
    const RELATIVE_OFFSET : usize = 24;

    while pointer >= GPS_HEADER_SIZE + RELATIVE_OFFSET {
        let active = data[pointer];
        let lon_hemi = data[pointer + 1];
        let lat_hemi = data[pointer + 2];

        // The A{N,S}{E,W} is 24 bytes away from the beginning of the data packet
	if active == b'A' 
            && (lon_hemi == b'N' || lon_hemi == b'S') 
            && (lat_hemi == b'E' || lat_hemi == b'W') 
        {
            return pointer.saturating_sub(RELATIVE_OFFSET);
        }

        pointer -= 1;
    }

    0
}
/// Write a diagnostic report for a NovaTek `.MOV` file so a tester at a
/// known location can mail it back if they notice the decoded track is
/// wrong (e.g. for clips recorded far from the original anchor point).
///
/// Report goes to `%USERPROFILE%\Dashcam-GPS-Debug\{basename}.txt` (or the
/// equivalent `$HOME/...` on non-Windows). Returns the output path.
pub fn dump_debug(path: &Path) -> Result<PathBuf, AppError> {
    use std::io::Write;

    let out_dir = default_debug_dir();
    std::fs::create_dir_all(&out_dir)?;

    let basename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("novatek");
    let out_path = out_dir.join(format!("{basename}.gpsdebug.txt"));

    let body = read_gps_atoms(path)?;
    let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut out = std::fs::File::create(&out_path)?;
    writeln!(out, "== Novatek GPS debug report ==")?;
    writeln!(out, "source        : {}", path.display())?;
    writeln!(out, "file size     : {file_len} bytes")?;

    let body = match body {
        Some(b) => b,
        None => {
            writeln!(out, "gps atom     : NOT FOUND")?;
            writeln!(
                out,
                "\nNo gps atom was present in this file. Please confirm the camera \
                 use NovaTek chipset and that GPS was enabled during recording."
            )?;
            return Ok(out_path);
        }
    };

    writeln!(out, "gps body     : {} bytes", body.len())?;
    let recs = body.len() / GPS_RECORD_SIZE;
    writeln!(
        out,
        "record count  : {recs} ({}B/record)",
        GPS_RECORD_SIZE
    )?;

    let sample_ct = recs.min(10);
    writeln!(
        out,
        "\n── First {sample_ct} records: raw + decoded ──────────────────────\n"
    )?;
    for idx in 0..sample_ct {
        let rec = &body[idx * GPS_RECORD_SIZE..(idx + 1) * GPS_RECORD_SIZE];
        writeln!(out, "record {idx}:")?;
        writeln!(out, "  hex     : {}", hex_block(rec))?;

        let lat_raw = f32::from_le_bytes(rec[OFF_LAT_F64..OFF_LAT_F64 + 4].try_into().unwrap()) as f64;
        let lon_raw = f32::from_le_bytes(rec[OFF_LON_F64..OFF_LON_F64 + 4].try_into().unwrap()) as f64;
        let lat = fix_coordinates(rec[OFF_LAT_HEMI], lat_raw);
        let lon = fix_coordinates(rec[OFF_LON_HEMI], lon_raw);

        let speed_knots = f32::from_le_bytes(rec[OFF_SPEED_KNOTS..OFF_SPEED_KNOTS + 4].try_into().unwrap()) as f64;
	let speed_mps = fix_speed(speed_knots);
	let bearing = f32::from_le_bytes(rec[OFF_BEARING..OFF_BEARING + 4].try_into().unwrap()) as f64;

        writeln!(
            out,
            "  decoded : lat={:.6}  lon={:.6}  speed={} m/s bearing={}",
            lat,
            lon,
            speed_mps,
	    bearing
        )?;

        if let Some(ts) = decode_timestamp(rec) {
            writeln!(out, "  time    : {ts} UTC")?;
        } else {
            writeln!(out, "  time    : INVALID (bytes {:02x?})", &rec[22..28])?;
        }
    }

    // Summary across the whole track.
    let mut min_ts: Option<NaiveDateTime> = None;
    let mut max_ts: Option<NaiveDateTime> = None;
    let (mut min_lat, mut max_lat) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut min_lon, mut max_lon) = (f64::INFINITY, f64::NEG_INFINITY);
    for idx in 0..recs {
        let rec = &body[idx * GPS_RECORD_SIZE..(idx + 1) * GPS_RECORD_SIZE];
        if let Some(ts) = decode_timestamp(rec) {
            min_ts = Some(min_ts.map_or(ts, |m| m.min(ts)));
            max_ts = Some(max_ts.map_or(ts, |m| m.max(ts)));
        }
        let lat_raw = f32::from_le_bytes(rec[OFF_LAT_F64..OFF_LAT_F64 + 4].try_into().unwrap()) as f64;
        let lon_raw = f32::from_le_bytes(rec[OFF_LON_F64..OFF_LON_F64 + 4].try_into().unwrap()) as f64;
        let lat = fix_coordinates(rec[OFF_LAT_HEMI], lat_raw);
        let lon = fix_coordinates(rec[OFF_LON_HEMI], lon_raw);
        if lat.is_finite() {
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
        }
        if lon.is_finite() {
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
        }
    }
    writeln!(out, "\n── Track summary ───────────────────────────────────────")?;
    writeln!(out, "time range    : {min_ts:?} → {max_ts:?}")?;
    writeln!(
        out,
        "lat range     : {min_lat:.6} → {max_lat:.6}"
    )?;
    writeln!(
        out,
        "lon range     : {min_lon:.6} → {max_lon:.6}"
    )?;

    Ok(out_path)
}

fn hex_block(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 8 == 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(windows)]
fn default_debug_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Dashcam-GPS-Debug")
}

#[cfg(not(windows))]
fn default_debug_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Dashcam-GPS-Debug")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_range_checks_basic() {
        assert!(in_range_lat(45.0));
        assert!(!in_range_lat(91.0));
        assert!(in_range_lon(-180.0));
        assert!(!in_range_lon(200.0));
        assert!(!in_range_lat(f64::NAN));
    }

    #[test]
    fn decode_timestamp_matches_sample_record() {
        let mut rec = [0u8; 44];
        rec[0] = 0x14; // hh = 20
        rec[4] = 0x0f; // mm = 15
        rec[8] = 0x04; // ss = 4
        rec[12] = 0x15; // yy = 21
        rec[16] = 0x0c; // mo = 12
        rec[20] = 0x02; // da = 2
        let ts = decode_timestamp(&rec).unwrap();
        assert_eq!(
            ts,
            NaiveDate::from_ymd_opt(2021, 12, 2)
                .unwrap()
                .and_hms_opt(20, 15, 4)
                .unwrap()
        );
    }

    /// Reconstructs the exact 44-byte record from the user's sample file and
    /// asserts the full pipeline (timestamp + lat + lon + speed + bearer) decodes
    /// correctly.
    #[test]
    fn decodes_record_3_end_to_end() {
        let rec_hex = "0000000023000000 070000001a000000 080000000e000000 414e450035149d45 30bc3c4433339a42 14ee1a43";
        let rec: Vec<u8> = rec_hex
            .split_whitespace()
            .collect::<String>()
            .as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(rec.len(), 44);

        // Timestamp: UTC 00:35:07 on 2026-08-14
        assert_eq!(
            decode_timestamp(&rec).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 14)
                .unwrap()
                .and_hms_opt(0, 35, 07)
                .unwrap()
        );

        // Lat/lon N50.442098  E7.915674
        let lat_raw = f32::from_le_bytes(rec[OFF_LAT_F64..OFF_LAT_F64 + 4].try_into().unwrap()) as f64;
        let lon_raw = f32::from_le_bytes(rec[OFF_LON_F64..OFF_LON_F64 + 4].try_into().unwrap()) as f64;
        let lat = fix_coordinates(rec[OFF_LAT_HEMI], lat_raw);
        let lon = fix_coordinates(rec[OFF_LON_HEMI], lon_raw);
        assert!((lat - 50.442098).abs() < 0.000001);
        assert!((lon - 7.915674).abs() < 0.000001);

	// Speed 39.663631615020755 m/s bearing 154.92999267578125 deg
        let speed_knots = f32::from_le_bytes(rec[OFF_SPEED_KNOTS..OFF_SPEED_KNOTS + 4].try_into().unwrap()) as f64;
	let speed_mps = fix_speed(speed_knots);
	let bearing = f32::from_le_bytes(rec[OFF_BEARING..OFF_BEARING + 4].try_into().unwrap()) as f64;
        assert!((speed_mps - 39.663631615020755).abs() < 1e-10);
        assert!((bearing - 154.92999267578125).abs() < 1e-10);
    }
}
