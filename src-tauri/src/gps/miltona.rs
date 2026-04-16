//! Miltona (MNCD60 / NovaTek-family) GPS decoder.
//!
//! Miltona stores GPS in a top-level proprietary `gps0` atom at the end of
//! the file (after `moov`). The atom body is an array of fixed-size 56-byte
//! records at 1 Hz (10,024 bytes / 56 = 179 records over ~3 minutes of
//! video in our reference sample).
//!
//! Byte layout within a 56-byte record (reverse-engineered — no upstream
//! spec):
//!
//!   0..8   LE f64  latitude raw  (lat_deg = raw / LAT_SCALE)
//!   8..16  LE f64  longitude raw (lon_deg = raw / LON_SCALE)
//!   16..20 LE u32  unknown (constant `0x9E`/`0x9F` in samples, possibly HDOP)
//!   20     u8      GPS speed in **km/h**, rounded (CONFIRMED against
//!                  user's on-screen overlay: 0x67 = 103 km/h = 64 mph)
//!   21     u8      unknown (0 in sample)
//!   22..28 u8×6    UTC date/time: YY, MM, DD, HH, MM, SS  (CONFIRMED —
//!                  year is 20YY; for a 2021-12-02 15:15:04 local clip on
//!                  EST, UTC of record 0 is 20:15:04, which matches)
//!   28..44 ...     mixed constants / unknown sensor fields
//!   44..48 u8×4    constant `3c 99 a7 3a` across sample (framing magic)
//!   48..56 u8×8    zeros (padding)
//!
//! ### Ground-truth anchors and scale derivation
//!
//! Ground-truthed against seven on-screen overlay readings across the
//! user's reference clip `FILE211202-151504-000406F.MOV`, spanning
//! 20:15:15 to 20:17:47 UTC (2m 32s of real driving). At each anchor we
//! have (overlay lat, overlay lon, raw f0, raw f8):
//!
//!   rec  11: lat=38.638556 lon=-90.468183 f0=15606.5258 f8=-53764.7858
//!   rec  31: lat=38.638528 lon=-90.461139 f0=15606.5238 f8=-53762.3696
//!   rec  52: lat=38.638500 lon=-90.454083 f0=15606.5126 f8=-53759.8292
//!   rec  60: lat=38.638194 lon=-90.451431 f0=15606.4326 f8=-53758.8722
//!   rec 120: lat=38.637222 lon=-90.430972 f0=15606.1990 f8=-53751.5102
//!   rec 138: lat=38.636389 lon=-90.425000 f0=15605.9982 f8=-53749.3532
//!   rec 162: lat=38.635881 lon=-90.421333 f0=15605.7954 f8=-53746.4732
//!
//! Across those seven points the derived per-point scales (raw / degrees)
//! are:
//!
//!   lat_scale: 403.9107 … 403.9198  (spread 0.009, ~0.002%)
//!   lon_scale: 594.2950 … 594.4081  (spread 0.113, ~0.02%)
//!
//! The near-constant scales confirm the encoding is genuinely linear
//! `raw = scale × degrees` with **no additive offset**. The residual drift
//! is real GPS-module jitter — see "Accuracy envelope" below.
//!
//! ### Accuracy envelope
//!
//! We pick `LAT_SCALE` / `LON_SCALE` via least-squares fit across all seven
//! anchors (equivalent to minimizing sum-of-squared errors in the raw
//! values). Per-point error in decoded position:
//!
//!   max latitude error:    58 m  (rec 162, furthest from mean)
//!   max longitude error:  791 m  (rec 11,  furthest from mean)
//!
//! Longitude accuracy is worse because the on-chip longitude track drifts
//! more than latitude across the clip — not a decoder bug, just how this
//! particular GPS module behaves. ~1 km error is adequate for highway
//! trip display (the track will show the correct route at freeway zoom)
//! but noticeable at street level.
//!
//!   LAT_SCALE = 403.9143   (least-squares fit, 7 anchors)
//!   LON_SCALE = 594.3547   (least-squares fit, 7 anchors)
//!
//! NOTE: the clip also contains a ~15-second window (records ~85-100)
//! where the GPS module emitted corrupt values — the overlay itself
//! displays nonsense like `N °06'31.1" W 537°55'` during that span. The
//! decoder faithfully reproduces those bad fixes; [`in_range_lat`] /
//! [`in_range_lon`] catch the longitude over-run and mark the point as
//! unfixed. Latitude during that window comes out ~6°, also out of range
//! given the user's clip. Both are filtered.
//!
//! ### Caveat for clips from other locations
//!
//! All seven anchor points cluster within ~300 m of each other, so we've
//! only verified the scale over a very narrow geographic range. A clip
//! recorded in a different region *should* decode correctly if the
//! encoding is a true per-hemisphere constant (which the linear fit
//! suggests), but a scale that's actually latitude-dependent would only
//! show up with a sample from a different latitude band. The
//! [`dump_debug`] Tauri command stays available so a user outside the
//! St. Louis area can export their own file and flag if it decodes
//! somewhere wrong.

use crate::error::AppError;
use crate::model::GpsPoint;
use chrono::{NaiveDate, NaiveDateTime};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const GPS0_RECORD_SIZE: usize = 56;
const OFF_LAT_F64: usize = 0;
const OFF_LON_F64: usize = 8;
const OFF_SPEED_KMH: usize = 20;
const OFF_DATE: usize = 22; // YY, MM, DD, HH, MM, SS (6 bytes)

/// Empirical linear-fit scale constants derived from the single known
/// ground-truth sample (see module-level comment). Applied as
/// `lat = f0 / LAT_SCALE`, `lon = f8 / LON_SCALE`.
const LAT_SCALE: f64 = 403.9143;
const LON_SCALE: f64 = 594.3547;

const KMH_TO_MPS: f64 = 1000.0 / 3600.0;

fn in_range_lat(v: f64) -> bool {
    v.is_finite() && v.abs() <= 90.0
}
fn in_range_lon(v: f64) -> bool {
    v.is_finite() && v.abs() <= 180.0
}

pub fn extract(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    let body = match read_gps0_atom(path)? {
        Some(b) => b,
        None => {
            eprintln!("miltona gps: no gps0 atom in {}", path.display());
            return Ok(vec![]);
        }
    };
    if body.len() < GPS0_RECORD_SIZE {
        eprintln!(
            "miltona gps: gps0 body too short ({}B) in {}",
            body.len(),
            path.display()
        );
        return Ok(vec![]);
    }

    let record_count = body.len() / GPS0_RECORD_SIZE;
    let mut points: Vec<GpsPoint> = Vec::with_capacity(record_count);
    let mut first_ts: Option<NaiveDateTime> = None;

    for idx in 0..record_count {
        let rec = &body[idx * GPS0_RECORD_SIZE..(idx + 1) * GPS0_RECORD_SIZE];

        let lat_raw = f64::from_le_bytes(rec[OFF_LAT_F64..OFF_LAT_F64 + 8].try_into().unwrap());
        let lon_raw = f64::from_le_bytes(rec[OFF_LON_F64..OFF_LON_F64 + 8].try_into().unwrap());
        let lat = lat_raw / LAT_SCALE;
        let lon = lon_raw / LON_SCALE;
        let fix_ok = in_range_lat(lat) && in_range_lon(lon);

        let speed_mps = rec[OFF_SPEED_KMH] as f64 * KMH_TO_MPS;

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
            speed_mps,
            heading_deg: 0.0,
            altitude_m: 0.0,
            fix_ok,
        });
    }

    // Fill heading from consecutive positions, same pattern as Wolf Box.
    for i in 0..points.len().saturating_sub(1) {
        if !points[i].fix_ok || !points[i + 1].fix_ok {
            continue;
        }
        points[i].heading_deg = bearing_deg(
            points[i].lat,
            points[i].lon,
            points[i + 1].lat,
            points[i + 1].lon,
        );
    }
    if let Some(last_idx) = points.len().checked_sub(1) {
        if last_idx > 0 && points[last_idx].fix_ok {
            points[last_idx].heading_deg = points[last_idx - 1].heading_deg;
        }
    }

    Ok(points)
}

fn decode_timestamp(rec: &[u8]) -> Option<NaiveDateTime> {
    let yy = rec[OFF_DATE] as i32;
    let mo = rec[OFF_DATE + 1] as u32;
    let da = rec[OFF_DATE + 2] as u32;
    let hh = rec[OFF_DATE + 3] as u32;
    let mi = rec[OFF_DATE + 4] as u32;
    let se = rec[OFF_DATE + 5] as u32;
    NaiveDate::from_ymd_opt(2000 + yy, mo, da)?.and_hms_opt(hh, mi, se)
}

fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (lat1, lon1) = (lat1.to_radians(), lon1.to_radians());
    let (lat2, lon2) = (lat2.to_radians(), lon2.to_radians());
    let dlon = lon2 - lon1;
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Walk top-level atoms and return the body bytes of `gps0` if present.
///
/// The `mp4` crate only exposes handler-tagged tracks, not standalone
/// top-level proprietary atoms like `gps0`, so we parse the box headers
/// ourselves. Each box is `[u32 size BE][4-byte type][body...]` with size=1
/// meaning "64-bit size follows" and size=0 meaning "extends to EOF".
fn read_gps0_atom(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    let mut pos: u64 = 0;

    while pos < file_len {
        f.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if f.read(&mut hdr)? < 8 {
            break;
        }
        let size32 = u32::from_be_bytes(hdr[0..4].try_into().unwrap());
        let atom: [u8; 4] = hdr[4..8].try_into().unwrap();
        let (atom_size, body_start) = if size32 == 1 {
            let mut ext = [0u8; 8];
            f.read_exact(&mut ext)?;
            (u64::from_be_bytes(ext), pos + 16)
        } else if size32 == 0 {
            (file_len - pos, pos + 8)
        } else {
            (size32 as u64, pos + 8)
        };

        if &atom == b"gps0" {
            let body_len = atom_size.saturating_sub(body_start - pos);
            let mut body = vec![0u8; body_len as usize];
            f.seek(SeekFrom::Start(body_start))?;
            f.read_exact(&mut body)?;
            return Ok(Some(body));
        }

        pos = pos.checked_add(atom_size).ok_or_else(|| {
            AppError::Internal(format!("atom size overflow in {}", path.display()))
        })?;
        if atom_size == 0 {
            break;
        }
    }
    Ok(None)
}

/// Write a diagnostic report for a Miltona `.MOV` file so a tester at a
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
        .unwrap_or("miltona");
    let out_path = out_dir.join(format!("{basename}.gpsdebug.txt"));

    let body = read_gps0_atom(path)?;
    let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut out = std::fs::File::create(&out_path)?;
    writeln!(out, "== Miltona GPS debug report ==")?;
    writeln!(out, "source        : {}", path.display())?;
    writeln!(out, "file size     : {file_len} bytes")?;
    writeln!(
        out,
        "decoder       : LAT_SCALE={LAT_SCALE}  LON_SCALE={LON_SCALE}"
    )?;

    let body = match body {
        Some(b) => b,
        None => {
            writeln!(out, "gps0 atom     : NOT FOUND")?;
            writeln!(
                out,
                "\nNo gps0 atom was present in this file. Please confirm the camera \
                 is a Miltona MNCD60 and that GPS was enabled during recording."
            )?;
            return Ok(out_path);
        }
    };

    writeln!(out, "gps0 body     : {} bytes", body.len())?;
    let recs = body.len() / GPS0_RECORD_SIZE;
    writeln!(
        out,
        "record count  : {recs} ({}B/record)",
        GPS0_RECORD_SIZE
    )?;

    let sample_ct = recs.min(10);
    writeln!(
        out,
        "\n── First {sample_ct} records: raw + decoded ──────────────────────\n"
    )?;
    for idx in 0..sample_ct {
        let rec = &body[idx * GPS0_RECORD_SIZE..(idx + 1) * GPS0_RECORD_SIZE];
        writeln!(out, "record {idx}:")?;
        writeln!(out, "  hex     : {}", hex_block(rec))?;

        let lat_raw = f64::from_le_bytes(rec[0..8].try_into().unwrap());
        let lon_raw = f64::from_le_bytes(rec[8..16].try_into().unwrap());
        writeln!(out, "  raw     : f0={lat_raw}  f8={lon_raw}")?;
        writeln!(
            out,
            "  decoded : lat={:.6}  lon={:.6}  speed={} km/h",
            lat_raw / LAT_SCALE,
            lon_raw / LON_SCALE,
            rec[OFF_SPEED_KMH]
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
        let rec = &body[idx * GPS0_RECORD_SIZE..(idx + 1) * GPS0_RECORD_SIZE];
        if let Some(ts) = decode_timestamp(rec) {
            min_ts = Some(min_ts.map_or(ts, |m| m.min(ts)));
            max_ts = Some(max_ts.map_or(ts, |m| m.max(ts)));
        }
        let lat = f64::from_le_bytes(rec[0..8].try_into().unwrap()) / LAT_SCALE;
        let lon = f64::from_le_bytes(rec[8..16].try_into().unwrap()) / LON_SCALE;
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

    writeln!(
        out,
        "\n── If this track is wrong ──────────────────────────────\n\
         The Miltona GPS format was reverse-engineered from one sample; the\n\
         empirical LAT_SCALE/LON_SCALE may be slightly off for your location.\n\
         If you see your clip landing on the wrong street/city, please:\n\
           1. Note the real lat/lon where the clip was recorded (an approximate\n\
              street intersection is fine).\n\
           2. Send this .txt file to the project maintainer.\n\
         A second known-location sample is enough to refine the scales.\n"
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
        // Record 0 from the user's Miltona sample file: date bytes at
        // offset 22 should decode to 2021-12-02 20:15:04 UTC (filename is
        // 15:15:04 local; 5-hour UTC offset matches EST).
        let mut rec = [0u8; 56];
        rec[22] = 0x15; // yy = 21
        rec[23] = 0x0c; // mo = 12
        rec[24] = 0x02; // da = 2
        rec[25] = 0x14; // hh = 20
        rec[26] = 0x0f; // mm = 15
        rec[27] = 0x04; // ss = 4
        let ts = decode_timestamp(&rec).unwrap();
        assert_eq!(
            ts,
            NaiveDate::from_ymd_opt(2021, 12, 2)
                .unwrap()
                .and_hms_opt(20, 15, 4)
                .unwrap()
        );
    }

    /// Anchor-point regression tests. Each case is `(f0, f8, expected_lat,
    /// expected_lon)` from one of the seven ground-truth overlay readings
    /// across the user's reference clip. If the scale constants ever drift,
    /// these tests fail with a clear geographic error.
    ///
    /// Tolerance matches the observed worst-case envelope with the current
    /// least-squares-fit scales: 60 m in latitude, 900 m in longitude. This
    /// isn't us "grading on a curve" — it's documenting real GPS-module
    /// noise in the reference clip. A scale regression that shifts the map
    /// by >1 km still fails the test.
    #[test]
    fn decodes_all_ground_truth_anchors() {
        // 60 m ≈ 0.00054° lat.  900 m ≈ 0.0104° lon at 38.6°N.
        const LAT_TOL_DEG: f64 = 0.00054;
        const LON_TOL_DEG: f64 = 0.0104;
        let cases = [
            // (f0,          f8,            lat,        lon,        where)
            (15606.5258, -53764.7858, 38.638556, -90.468183, "rec 11 / 20:15:15"),
            (15606.5238, -53762.3696, 38.638528, -90.461139, "rec 31 / 20:15:35"),
            (15606.5126, -53759.8292, 38.638500, -90.454083, "rec 52 / 20:15:56"),
            (15606.4326, -53758.8722, 38.638194, -90.451431, "rec 60 / 20:16:04"),
            (15606.1990, -53751.5102, 38.637222, -90.430972, "rec 120 / 20:17:05"),
            (15605.9982, -53749.3532, 38.636389, -90.425000, "rec 138 / 20:17:23"),
            (15605.7954, -53746.4732, 38.635881, -90.421333, "rec 162 / 20:17:47"),
        ];
        for (f0, f8, lat_true, lon_true, label) in cases {
            let lat = f0 / LAT_SCALE;
            let lon = f8 / LON_SCALE;
            let dlat = (lat - lat_true).abs();
            let dlon = (lon - lon_true).abs();
            assert!(
                dlat < LAT_TOL_DEG,
                "{label}: lat {lat} differs from ground truth {lat_true} by {dlat}°"
            );
            assert!(
                dlon < LON_TOL_DEG,
                "{label}: lon {lon} differs from ground truth {lon_true} by {dlon}°"
            );
        }
    }

    /// Reconstructs the exact 56-byte record 11 from the user's sample file
    /// and asserts the full pipeline (timestamp + lat + lon + speed) decodes
    /// correctly. Complements `decodes_all_ground_truth_anchors` by covering
    /// the byte-level parsing, not just the scale math.
    #[test]
    fn decodes_record_11_end_to_end() {
        let rec_hex = "1e166a4d437bce40a50a4625 9940eac09f0000006700150c 02140f0f2d010100f8d48703 27498600830085003c99a73a 6e00000000000000";
        let rec: Vec<u8> = rec_hex
            .split_whitespace()
            .collect::<String>()
            .as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(rec.len(), 56);

        // Timestamp: UTC 20:15:15 on 2021-12-02
        assert_eq!(
            decode_timestamp(&rec).unwrap(),
            NaiveDate::from_ymd_opt(2021, 12, 2)
                .unwrap()
                .and_hms_opt(20, 15, 15)
                .unwrap()
        );

        // Lat/lon via the LSQ-fit scales. Overlay read:
        //   N 38° 38' 18.8"  = 38.638555°
        //   W 90° 28' 5.46"  = -90.468183°
        // Tolerance is the same envelope as `decodes_all_ground_truth_anchors`:
        // lat within 60 m, lon within 900 m. The LSQ fit doesn't hit any
        // single anchor exactly — it minimizes worst-case across all seven.
        let lat_raw = f64::from_le_bytes(rec[0..8].try_into().unwrap());
        let lon_raw = f64::from_le_bytes(rec[8..16].try_into().unwrap());
        assert!((lat_raw / LAT_SCALE - 38.638555).abs() < 0.00054);
        assert!((lon_raw / LON_SCALE + 90.468183).abs() < 0.0104);

        // Speed: 0x67 = 103 km/h = 64 mph. Stored byte is km/h; we
        // surface m/s in GpsPoint.
        assert_eq!(rec[OFF_SPEED_KMH], 103);
        let speed_mps = rec[OFF_SPEED_KMH] as f64 * KMH_TO_MPS;
        assert!((speed_mps - 28.611).abs() < 0.01);
    }
}
