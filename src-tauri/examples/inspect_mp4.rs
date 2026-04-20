//! Deep inspection of an MP4 file for recovery planning.
//!
//! Usage: `cargo run --example inspect_mp4 -- <path>`
//!
//! Prints the top-level box walk, a summary of moov if present, the first
//! bytes of mdat (to identify Wolf Box camera-metadata prefix), and a walk
//! of the first several length-prefixed HEVC NAL units found after that
//! prefix. Tolerates missing or malformed moov — the primary use case is
//! inspecting the broken files we're trying to recover.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let path_arg = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: inspect_mp4 <path>");
        std::process::exit(2);
    });
    let path = Path::new(&path_arg);
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {}: {e}", path.display());
        std::process::exit(1);
    });

    println!("==============================================================");
    println!("file: {}", path.display());
    println!("size: {} bytes ({:.1} MB)", bytes.len(), bytes.len() as f64 / 1_048_576.0);
    println!();

    let boxes = walk_top_level_boxes(&bytes);
    print_box_walk(&boxes, bytes.len());

    if boxes.iter().any(|b| b.typ == "moov") {
        println!();
        println!("--- moov ---");
        dump_moov(path);
    } else {
        println!();
        println!("--- moov ---");
        println!("no moov box found (No Index file)");
    }

    if let Some(mdat) = boxes.iter().find(|b| b.typ == "mdat") {
        println!();
        println!("--- mdat ---");
        dump_mdat(&bytes, mdat);
    }
}

#[derive(Debug, Clone)]
struct TopBox {
    typ: String,
    offset: u64,
    declared_size: u64,
    header_size: u64, // 8 or 16
}

fn walk_top_level_boxes(bytes: &[u8]) -> Vec<TopBox> {
    let mut out = Vec::new();
    let len = bytes.len() as u64;
    let mut pos: u64 = 0;

    while pos + 8 <= len {
        let p = pos as usize;
        let declared = u32::from_be_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]);
        let typ = std::str::from_utf8(&bytes[p + 4..p + 8])
            .unwrap_or("????")
            .to_string();
        let (size, header_size) = match declared {
            1 => {
                if pos + 16 > len {
                    break;
                }
                let big = u64::from_be_bytes([
                    bytes[p + 8], bytes[p + 9], bytes[p + 10], bytes[p + 11],
                    bytes[p + 12], bytes[p + 13], bytes[p + 14], bytes[p + 15],
                ]);
                (big, 16)
            }
            0 => (len - pos, 8),
            n => (u64::from(n), 8),
        };
        out.push(TopBox {
            typ: typ.clone(),
            offset: pos,
            declared_size: size,
            header_size,
        });
        // Advance by declared_size, but if that would push us beyond EOF
        // or to the same spot, bail — this file's box chain is malformed.
        let step = if size < header_size || size > len - pos {
            break;
        } else {
            size
        };
        pos += step;
    }
    out
}

fn print_box_walk(boxes: &[TopBox], file_len: usize) {
    println!("--- top-level boxes ---");
    println!("{:<6} {:<12} {:<12} {:<12} notes", "type", "offset", "decl_size", "header");
    let file_len = file_len as u64;
    for b in boxes {
        let mut notes = String::new();
        if b.offset + b.declared_size > file_len {
            notes.push_str(&format!(
                "DECL OVERSHOOTS EOF by {} ",
                b.offset + b.declared_size - file_len
            ));
        }
        if b.typ == "mdat" {
            let payload_len = b.declared_size.saturating_sub(b.header_size);
            let real_remaining = file_len.saturating_sub(b.offset + b.header_size);
            notes.push_str(&format!(
                "payload_decl={} real_remaining={} ",
                payload_len, real_remaining
            ));
            if payload_len != real_remaining {
                notes.push_str(&format!(
                    "SIZE MISMATCH ({} vs {}) ",
                    payload_len, real_remaining
                ));
            }
        }
        println!(
            "{:<6} {:<12} {:<12} {:<12} {}",
            b.typ, b.offset, b.declared_size, b.header_size, notes
        );
    }
    // What happens past the last box's declared end?
    if let Some(last) = boxes.last() {
        let end = last.offset + last.declared_size;
        if end < file_len {
            println!(
                "(tail after last box: {} bytes from offset {})",
                file_len - end,
                end
            );
        }
    }
}

fn dump_moov(path: &Path) {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            println!("could not reopen for mp4 crate: {e}");
            return;
        }
    };
    let size = match f.metadata() {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    let reader = std::io::BufReader::new(f);
    let mp4 = match mp4::Mp4Reader::read_header(reader, size) {
        Ok(m) => m,
        Err(e) => {
            println!("mp4 crate parse failed: {e}");
            return;
        }
    };

    println!(
        "major_brand: {}  timescale: {}  duration: {:?}",
        mp4.ftyp.major_brand, mp4.moov.mvhd.timescale, mp4.duration()
    );
    println!("tracks:");
    for t in mp4.tracks().values() {
        let box_type = t.box_type().map(|b| b.to_string()).unwrap_or_else(|_| "?".into());
        let track_type = t.track_type().map(|tt| format!("{tt:?}")).unwrap_or_else(|_| "?".into());
        let sample_count = t.sample_count();
        let w = t.width();
        let h = t.height();
        let fps = t.frame_rate();
        println!(
            "  #{}  type={}  box={}  samples={}  {}x{}@{:.2}fps  timescale={}  duration={:?}",
            t.track_id(),
            track_type,
            box_type,
            sample_count,
            w,
            h,
            fps,
            t.timescale(),
            t.duration(),
        );
        // Sample-size histogram via the public stsz box on the track.
        let stsz = &t.trak.mdia.minf.stbl.stsz;
        if !stsz.sample_sizes.is_empty() {
            let mut sizes = stsz.sample_sizes.clone();
            let total: u64 = sizes.iter().map(|&s| u64::from(s)).sum();
            sizes.sort_unstable();
            let min = sizes.first().copied().unwrap_or(0);
            let max = sizes.last().copied().unwrap_or(0);
            let median = sizes[sizes.len() / 2];
            println!(
                "     sample sizes: min={} median={} max={} total={} (from stsz[{}])",
                min, median, max, total, stsz.sample_sizes.len()
            );
        } else if stsz.sample_size > 0 {
            println!(
                "     sample size: fixed {} × {} = {}",
                stsz.sample_size, stsz.sample_count, u64::from(stsz.sample_size) * u64::from(stsz.sample_count)
            );
        }
        // Chunk offsets live in either stco (32-bit) or co64 (64-bit).
        let stbl = &t.trak.mdia.minf.stbl;
        if let Some(stco) = &stbl.stco {
            if let (Some(first), Some(last)) = (stco.entries.first(), stco.entries.last()) {
                println!(
                    "     chunk offsets (stco): first=0x{:x} last=0x{:x} count={}",
                    first, last, stco.entries.len()
                );
            }
        }
        if let Some(co64) = &stbl.co64 {
            if let (Some(first), Some(last)) = (co64.entries.first(), co64.entries.last()) {
                println!(
                    "     chunk offsets (co64): first=0x{:x} last=0x{:x} count={}",
                    first, last, co64.entries.len()
                );
            }
        }
    }
}

fn dump_mdat(bytes: &[u8], mdat: &TopBox) {
    let file_len = bytes.len() as u64;
    let payload_start = mdat.offset + mdat.header_size;
    let payload_end = file_len; // ignore declared size — we know it's often wrong
    let payload_len = payload_end - payload_start;
    println!(
        "payload range: 0x{:x}..0x{:x}  ({} bytes)",
        payload_start, payload_end, payload_len
    );

    let slice = &bytes[payload_start as usize..payload_end as usize];
    let preview_len = slice.len().min(256);
    println!("first {} bytes of mdat payload:", preview_len);
    print_hex_ascii(&slice[..preview_len], payload_start);

    // Try to identify a Wolf Box 'camb'-style prefix. It looks like a
    // standard ISO box header: 4-byte BE size + 4-byte ASCII type.
    let prefix = detect_prefix_box(slice);
    match prefix {
        Some((size, typ)) if typ.chars().all(|c| c.is_ascii_alphanumeric()) && size > 0 && size <= slice.len() as u64 => {
            println!(
                "detected prefix box: type='{}' size={} (would-skip to 0x{:x})",
                typ,
                size,
                payload_start + size,
            );
            walk_nals(&slice[size as usize..], payload_start + size, 12);
        }
        _ => {
            println!("no clear prefix box at mdat start — walking NALs from payload start");
            walk_nals(slice, payload_start, 12);
        }
    }
}

fn detect_prefix_box(slice: &[u8]) -> Option<(u64, String)> {
    if slice.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]);
    let typ = std::str::from_utf8(&slice[4..8]).ok()?.to_string();
    let size = match size {
        0 => slice.len() as u64,
        1 => {
            if slice.len() < 16 { return None; }
            u64::from_be_bytes([
                slice[8], slice[9], slice[10], slice[11],
                slice[12], slice[13], slice[14], slice[15],
            ])
        }
        n => u64::from(n),
    };
    Some((size, typ))
}

fn walk_nals(slice: &[u8], absolute_start: u64, limit: usize) {
    println!("first {} length-prefixed NALs (from 0x{:x}):", limit, absolute_start);
    let mut pos: usize = 0;
    let mut count = 0;
    while pos + 4 <= slice.len() && count < limit {
        let nal_len = u32::from_be_bytes([
            slice[pos], slice[pos + 1], slice[pos + 2], slice[pos + 3],
        ]) as usize;
        if nal_len == 0 || pos + 4 + nal_len > slice.len() {
            println!(
                "  [halt] bad NAL length {} at offset 0x{:x} (remaining {})",
                nal_len,
                absolute_start + pos as u64,
                slice.len() - pos,
            );
            break;
        }
        let header_byte = slice[pos + 4];
        let nal_type = (header_byte >> 1) & 0x3f;
        let name = hevc_nal_name(nal_type);
        println!(
            "  [{:>3}] len={:<8} type={:>2} ({}) @ 0x{:x}",
            count,
            nal_len,
            nal_type,
            name,
            absolute_start + pos as u64,
        );
        pos += 4 + nal_len;
        count += 1;
    }
    if count == limit {
        println!("  … (stopping at {} NALs)", limit);
    }
}

fn hevc_nal_name(t: u8) -> &'static str {
    match t {
        0 => "TRAIL_N",
        1 => "TRAIL_R",
        2 => "TSA_N",
        3 => "TSA_R",
        4 => "STSA_N",
        5 => "STSA_R",
        6 => "RADL_N",
        7 => "RADL_R",
        8 => "RASL_N",
        9 => "RASL_R",
        16..=18 => "BLA_*",
        19 => "IDR_W_RADL",
        20 => "IDR_N_LP",
        21 => "CRA",
        22 => "RSV_IRAP_22",
        23 => "RSV_IRAP_23",
        32 => "VPS",
        33 => "SPS",
        34 => "PPS",
        35 => "AUD",
        36 => "EOS",
        37 => "EOB",
        38 => "FD",
        39 => "PREFIX_SEI",
        40 => "SUFFIX_SEI",
        _ => "?",
    }
}

fn print_hex_ascii(bytes: &[u8], base: u64) {
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let addr = base + (i * 16) as u64;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
        let hex_pad = format!("{:<48}", hex.join(" "));
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("  {:08x}: {}  {}", addr, hex_pad, ascii);
    }
}
