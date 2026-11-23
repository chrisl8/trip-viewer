//! 70mai (4K T800) GPS decoder.
//!
//! The T800 stores one small telemetry record per video frame in an extra
//! MP4 track whose handler is `hint` (sample entry `rtp `). It is not a real
//! RTP track, just a carrier: a 60 s clip at 25 fps has 1500 samples of
//! 493 bytes each, all in a single chunk. The GPS receiver updates once per
//! second, so each record repeats ~25 times before the position changes.
//!
//! Byte layout of a sample (reverse-engineered — no upstream spec; all
//! integers are little-endian i32):
//!
//!   14      u8   fix status: `'A'` = valid, anything else = no fix
//!   15..19  i32  latitude,  NMEA `ddmm.mmmmm` × 1e5   (45°56.50733' → 455650733)
//!   19..23  i32  longitude, NMEA `dddmm.mmmmm` × 1e5
//!   23..27  i32  heading in degrees (0–359)
//!   27..31  i32  speed in km/h
//!   31..35  i32  unknown, drifts slowly (altitude? not used)
//!
//! The rest of the sample holds accelerometer-style values, a Unix
//! timestamp and a constant `{pA:V,rA:V,sF:0,tF:0}` status string.
//!
//! ### Ground truth
//!
//! Checked against the burned-in overlay (position, speed) of one real
//! 60 s front clip at five moments. Position and speed matched to the
//! overlay's precision at the moments the overlay and sample line up;
//! elsewhere they differ by less than one second of travel, which is
//! consistent with the overlay lagging the telemetry slightly. Heading and
//! speed also agree with the bearing/speed derived from consecutive
//! positions.
//!
//! ### Not verified
//!
//! Only northern/eastern hemisphere footage was available. Southern and
//! western positions are assumed to be stored as negative values; if a
//! sample shows otherwise, the sign handling in `nmea_to_degrees` is the
//! place to change.
//!
//! Timing: the hint track's `stts` deltas are all zero, so sample times are
//! derived as `index × movie_duration / sample_count`.

use crate::error::AppError;
use crate::model::GpsPoint;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const OFF_FIX: usize = 14;
const OFF_LAT: usize = 15;
const OFF_LON: usize = 19;
const OFF_HEADING: usize = 23;
const OFF_SPEED_KMH: usize = 27;
/// Bytes of each sample we need (up to and including the speed field).
const SAMPLE_HEAD_LEN: usize = 31;
const KMH_TO_MPS: f64 = 1000.0 / 3600.0;
const FALLBACK_FPS: f64 = 25.0;
/// Refuse absurd box sizes / sample counts from corrupt files.
const MAX_MOOV_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SAMPLES: usize = 500_000;

pub fn extract(path: &Path) -> Result<Vec<GpsPoint>, AppError> {
    let mut file = File::open(path)?;
    let moov = match read_moov(&mut file)? {
        Some(m) => m,
        None => {
            eprintln!("70mai gps: no moov box in {}", path.display());
            return Ok(vec![]);
        }
    };

    let samples = match find_hint_samples(&moov) {
        Some(s) if !s.is_empty() => s,
        _ => {
            eprintln!("70mai gps: no telemetry track in {}", path.display());
            return Ok(vec![]);
        }
    };

    let duration_s = movie_duration_s(&moov).unwrap_or(samples.len() as f64 / FALLBACK_FPS);
    let step = duration_s / samples.len() as f64;

    let mut points: Vec<GpsPoint> = Vec::new();
    let mut last_key: Option<(i32, i32, bool)> = None;
    let mut head = [0u8; SAMPLE_HEAD_LEN];

    for (idx, &(offset, size)) in samples.iter().enumerate() {
        if (size as usize) < SAMPLE_HEAD_LEN {
            continue;
        }
        if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut head).is_err() {
            // Truncated file: keep what we have.
            break;
        }

        let lat_raw = le_i32(&head, OFF_LAT);
        let lon_raw = le_i32(&head, OFF_LON);
        let fix = head[OFF_FIX] == b'A';
        let key = (lat_raw, lon_raw, fix);
        if last_key == Some(key) {
            continue; // same GPS update repeated across frames
        }
        last_key = Some(key);

        let lat = nmea_to_degrees(lat_raw);
        let lon = nmea_to_degrees(lon_raw);

        // No fix (status != 'A', all-zero position while the receiver is
        // still acquiring, out-of-range values): drop the point instead of
        // emitting it. The map and the stitched trip track draw every point
        // they get, so a (0, 0) point would pull the route to the Gulf of
        // Guinea. This matches the other decoders, which only emit usable
        // positions.
        if !(fix && in_range(lat, lon)) {
            continue;
        }

        let heading = le_i32(&head, OFF_HEADING);
        let speed_kmh = le_i32(&head, OFF_SPEED_KMH);

        points.push(GpsPoint {
            t_offset_s: idx as f64 * step,
            lat,
            lon,
            speed_mps: speed_kmh.max(0) as f64 * KMH_TO_MPS,
            heading_deg: if (0..360).contains(&heading) { heading as f64 } else { 0.0 },
            altitude_m: 0.0,
            fix_ok: true,
        });
    }

    Ok(points)
}

fn le_i32(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Convert the camera's `ddmm.mmmmm × 1e5` integer to signed decimal
/// degrees. Works for both latitude (2-digit degrees) and longitude
/// (3-digit degrees) because minutes are always the last two integer digits.
fn nmea_to_degrees(raw: i32) -> f64 {
    let sign = if raw < 0 { -1.0 } else { 1.0 };
    let v = (raw as i64).abs() as f64 / 1e5; // ddmm.mmmmm
    let degrees = (v / 100.0).floor();
    let minutes = v - degrees * 100.0;
    sign * (degrees + minutes / 60.0)
}

fn in_range(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && !(lat == 0.0 && lon == 0.0)
}

// ── Minimal MP4 reading ─────────────────────────────────────────────────────
//
// Just enough of ISO-BMFF to locate the `hint` track's sample table. Kept
// dependency-free on purpose; the `mp4` crate does not expose hint tracks.

/// Walk top-level boxes and return the body of `moov`.
fn read_moov(file: &mut File) -> Result<Option<Vec<u8>>, AppError> {
    let file_len = file.metadata()?.len();
    let mut pos: u64 = 0;
    while pos + 8 <= file_len {
        file.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        file.read_exact(&mut hdr)?;
        let mut size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let mut header_len = 8u64;
        if size == 1 {
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext)?;
            size = u64::from_be_bytes(ext);
            header_len = 16;
        } else if size == 0 {
            size = file_len - pos;
        }
        if size < header_len {
            return Ok(None);
        }
        if &typ == b"moov" {
            let body_len = size - header_len;
            if body_len > MAX_MOOV_BYTES || pos + size > file_len {
                return Ok(None);
            }
            let mut body = vec![0u8; body_len as usize];
            file.read_exact(&mut body)?;
            return Ok(Some(body));
        }
        pos += size;
    }
    Ok(None)
}

/// Iterate the child boxes of a container body as `(type, body)`.
fn children(buf: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= buf.len() {
        let mut size = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as u64;
        let typ = [buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]];
        let mut header_len = 8usize;
        if size == 1 {
            if pos + 16 > buf.len() {
                break;
            }
            size = u64::from_be_bytes(buf[pos + 8..pos + 16].try_into().unwrap());
            header_len = 16;
        } else if size == 0 {
            size = (buf.len() - pos) as u64;
        }
        let end = pos as u64 + size;
        if size < header_len as u64 || end > buf.len() as u64 {
            break;
        }
        out.push((typ, &buf[pos + header_len..end as usize]));
        pos = end as usize;
    }
    out
}

fn child<'a>(buf: &'a [u8], typ: &[u8; 4]) -> Option<&'a [u8]> {
    children(buf).into_iter().find(|(t, _)| t == typ).map(|(_, b)| b)
}

fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

fn movie_duration_s(moov: &[u8]) -> Option<f64> {
    let mvhd = child(moov, b"mvhd")?;
    let (timescale, duration) = if *mvhd.first()? == 1 {
        (be_u32(mvhd, 20)? as f64, be_u64(mvhd, 24)? as f64)
    } else {
        (be_u32(mvhd, 12)? as f64, be_u32(mvhd, 16)? as f64)
    };
    if timescale > 0.0 && duration > 0.0 {
        Some(duration / timescale)
    } else {
        None
    }
}

/// Find the first `hint` track and return `(file_offset, size)` per sample.
fn find_hint_samples(moov: &[u8]) -> Option<Vec<(u64, u32)>> {
    for (typ, trak) in children(moov) {
        if &typ != b"trak" {
            continue;
        }
        if let Some(samples) = hint_track_samples(trak) {
            return Some(samples);
        }
    }
    None
}

fn hint_track_samples(trak: &[u8]) -> Option<Vec<(u64, u32)>> {
    let mdia = child(trak, b"mdia")?;
    let hdlr = child(mdia, b"hdlr")?;
    if hdlr.get(8..12)? != b"hint" {
        return None;
    }
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;

    // Sample sizes.
    let stsz = child(stbl, b"stsz")?;
    let uniform = be_u32(stsz, 4)?;
    let count = be_u32(stsz, 8)? as usize;
    if count == 0 || count > MAX_SAMPLES {
        return None;
    }
    let mut sizes: Vec<u32> = Vec::with_capacity(count);
    if uniform != 0 {
        sizes.resize(count, uniform);
    } else {
        for i in 0..count {
            sizes.push(be_u32(stsz, 12 + 4 * i)?);
        }
    }

    // Sample → chunk mapping.
    let stsc = child(stbl, b"stsc")?;
    let entry_count = be_u32(stsc, 4)? as usize;
    let mut stsc_entries: Vec<(u32, u32)> = Vec::with_capacity(entry_count.min(1024));
    for i in 0..entry_count.min(1024) {
        let base = 8 + 12 * i;
        stsc_entries.push((be_u32(stsc, base)?, be_u32(stsc, base + 4)?));
    }
    if stsc_entries.is_empty() {
        return None;
    }

    // Chunk offsets (32- or 64-bit).
    let chunk_offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        let n = be_u32(stco, 4)? as usize;
        if n > MAX_SAMPLES {
            return None;
        }
        (0..n).map(|i| be_u32(stco, 8 + 4 * i).map(u64::from)).collect::<Option<_>>()?
    } else {
        let co64 = child(stbl, b"co64")?;
        let n = be_u32(co64, 4)? as usize;
        if n > MAX_SAMPLES {
            return None;
        }
        (0..n).map(|i| be_u64(co64, 8 + 8 * i)).collect::<Option<_>>()?
    };

    let mut out: Vec<(u64, u32)> = Vec::with_capacity(count);
    let mut sample_idx = 0usize;
    for (chunk_idx, &chunk_off) in chunk_offsets.iter().enumerate() {
        let chunk_no = chunk_idx as u32 + 1;
        let per_chunk = stsc_entries
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk_no)
            .map(|(_, spc)| *spc)?;
        let mut offset = chunk_off;
        for _ in 0..per_chunk {
            if sample_idx >= count {
                return Some(out);
            }
            let size = sizes[sample_idx];
            out.push((offset, size));
            offset += size as u64;
            sample_idx += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn mp4_box(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(8 + body.len());
        v.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
        v.extend_from_slice(typ);
        v.extend_from_slice(body);
        v
    }

    /// One 493-byte telemetry sample with invented values.
    fn sample(fix: u8, lat_raw: i32, lon_raw: i32, heading: i32, speed_kmh: i32) -> Vec<u8> {
        let mut s = vec![0u8; 493];
        s[OFF_FIX] = fix;
        s[OFF_LAT..OFF_LAT + 4].copy_from_slice(&lat_raw.to_le_bytes());
        s[OFF_LON..OFF_LON + 4].copy_from_slice(&lon_raw.to_le_bytes());
        s[OFF_HEADING..OFF_HEADING + 4].copy_from_slice(&heading.to_le_bytes());
        s[OFF_SPEED_KMH..OFF_SPEED_KMH + 4].copy_from_slice(&speed_kmh.to_le_bytes());
        s
    }

    fn build_moov(handler: &[u8; 4], sample_count: u32, duration_ms: u32, data_offset: u32) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd[16..20].copy_from_slice(&duration_ms.to_be_bytes());

        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(handler);

        let mut stsz = vec![0u8; 12];
        stsz[4..8].copy_from_slice(&493u32.to_be_bytes());
        stsz[8..12].copy_from_slice(&sample_count.to_be_bytes());

        let mut stsc = vec![0u8; 8];
        stsc[4..8].copy_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        stsc.extend_from_slice(&sample_count.to_be_bytes()); // samples per chunk
        stsc.extend_from_slice(&1u32.to_be_bytes()); // description

        let mut stco = vec![0u8; 8];
        stco[4..8].copy_from_slice(&1u32.to_be_bytes());
        stco.extend_from_slice(&data_offset.to_be_bytes());

        let stbl = mp4_box(
            b"stbl",
            &[mp4_box(b"stsz", &stsz), mp4_box(b"stsc", &stsc), mp4_box(b"stco", &stco)].concat(),
        );
        let minf = mp4_box(b"minf", &stbl);
        let mdia = mp4_box(b"mdia", &[mp4_box(b"hdlr", &hdlr), minf].concat());
        let trak = mp4_box(b"trak", &mdia);
        mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), trak].concat())
    }

    fn write_clip(handler: &[u8; 4], samples: &[Vec<u8>], duration_ms: u32) -> tempfile::NamedTempFile {
        let moov_len = build_moov(handler, samples.len() as u32, duration_ms, 0).len();
        let moov = build_moov(handler, samples.len() as u32, duration_ms, moov_len as u32);
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&moov).unwrap();
        for s in samples {
            f.write_all(s).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn nmea_conversion() {
        // 45°30.00000' → 45.5, 010°30.00000' → 10.5
        assert!((nmea_to_degrees(453_000_000) - 45.5).abs() < 1e-9);
        assert!((nmea_to_degrees(103_000_000) - 10.5).abs() < 1e-9);
        // 3-digit degrees: 123°15.00000' → 123.25
        assert!((nmea_to_degrees(1_231_500_000) - 123.25).abs() < 1e-9);
        // Negative = south/west.
        assert!((nmea_to_degrees(-453_000_000) + 45.5).abs() < 1e-9);
        assert_eq!(nmea_to_degrees(0), 0.0);
    }

    #[test]
    fn decodes_distinct_updates_with_times() {
        // 6 frames, 3 distinct positions (each repeated twice), 3 s long.
        let a = sample(b'A', 453_000_000, 103_000_000, 90, 36);
        let b = sample(b'A', 453_030_000, 103_030_000, 180, 72);
        let c = sample(b'A', 453_060_000, 103_060_000, 270, 0);
        let clip = write_clip(b"hint", &[a.clone(), a, b.clone(), b, c.clone(), c], 3000);

        let pts = extract(clip.path()).unwrap();
        assert_eq!(pts.len(), 3);

        assert!((pts[0].lat - 45.5).abs() < 1e-9);
        assert!((pts[0].lon - 10.5).abs() < 1e-9);
        assert!((pts[0].speed_mps - 10.0).abs() < 1e-9); // 36 km/h
        assert_eq!(pts[0].heading_deg, 90.0);
        assert_eq!(pts[0].t_offset_s, 0.0);

        assert!((pts[1].lat - 45.505).abs() < 1e-9); // 30.00' + 0.30' = 30.30'
        assert!((pts[1].speed_mps - 20.0).abs() < 1e-9); // 72 km/h
        assert!((pts[1].t_offset_s - 1.0).abs() < 1e-9);
        assert!((pts[2].t_offset_s - 2.0).abs() < 1e-9);
        assert!(pts.iter().all(|p| p.fix_ok));
    }

    #[test]
    fn negative_values_are_south_west() {
        let s = sample(b'A', -453_000_000, -103_000_000, 0, 0);
        let clip = write_clip(b"hint", &[s], 1000);
        let pts = extract(clip.path()).unwrap();
        assert_eq!(pts.len(), 1);
        assert!((pts[0].lat + 45.5).abs() < 1e-9);
        assert!((pts[0].lon + 10.5).abs() < 1e-9);
        assert!(pts[0].fix_ok);
    }

    #[test]
    fn void_fix_is_skipped() {
        let s = sample(b'V', 453_000_000, 103_000_000, 0, 0);
        let clip = write_clip(b"hint", &[s], 1000);
        assert!(extract(clip.path()).unwrap().is_empty());
    }

    #[test]
    fn zero_position_is_skipped() {
        let s = sample(b'A', 0, 0, 0, 0);
        let clip = write_clip(b"hint", &[s], 1000);
        assert!(extract(clip.path()).unwrap().is_empty());
    }

    #[test]
    fn leading_no_fix_samples_do_not_shift_valid_times() {
        // 4 frames over 4 s: two with no position yet, then two valid
        // updates. Only the valid ones come out, with their true times.
        let none = sample(b'A', 0, 0, 0, 0);
        let a = sample(b'A', 453_000_000, 103_000_000, 0, 0);
        let b = sample(b'A', 453_030_000, 103_030_000, 0, 0);
        let clip = write_clip(b"hint", &[none.clone(), none, a, b], 4000);
        let pts = extract(clip.path()).unwrap();
        assert_eq!(pts.len(), 2);
        assert!((pts[0].t_offset_s - 2.0).abs() < 1e-9);
        assert!((pts[1].t_offset_s - 3.0).abs() < 1e-9);
        assert!(pts.iter().all(|p| p.fix_ok && p.lat > 0.0));
    }

    #[test]
    fn file_without_hint_track_yields_nothing() {
        let s = sample(b'A', 453_000_000, 103_000_000, 0, 0);
        let clip = write_clip(b"vide", &[s], 1000);
        assert!(extract(clip.path()).unwrap().is_empty());
    }

    #[test]
    fn truncated_file_keeps_what_was_read() {
        let a = sample(b'A', 453_000_000, 103_000_000, 0, 0);
        let b = sample(b'A', 453_030_000, 103_030_000, 0, 0);
        let clip = write_clip(b"hint", &[a, b], 2000);
        // Cut into the last sample's head so it can no longer be read.
        let len = std::fs::metadata(clip.path()).unwrap().len();
        clip.as_file().set_len(len - 480).unwrap();
        let pts = extract(clip.path()).unwrap();
        assert_eq!(pts.len(), 1);
    }

    #[test]
    fn garbage_file_is_not_an_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"this is not an mp4 file at all").unwrap();
        f.flush().unwrap();
        assert!(extract(f.path()).unwrap().is_empty());
    }
}
