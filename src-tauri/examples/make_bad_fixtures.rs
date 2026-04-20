//! Produce a folder of intentionally-broken MP4 files that exercise each
//! `ScanErrorKind` the scanner can classify. Useful for verifying the
//! IssuesView renders each category correctly without having to wait for
//! a real SD card to fail in the wild.
//!
//! Usage:
//!
//! ```text
//! cargo run --manifest-path src-tauri/Cargo.toml \
//!   --example make_bad_fixtures -- <good.mp4> <out_dir>
//! ```
//!
//! The donor file is only read, never modified. The output folder is
//! created if missing. Existing files with colliding names are overwritten.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let donor = args.next().unwrap_or_else(|| {
        eprintln!("usage: make_bad_fixtures <good.mp4> <out_dir>");
        std::process::exit(2);
    });
    let out_dir = args.next().unwrap_or_else(|| {
        eprintln!("usage: make_bad_fixtures <good.mp4> <out_dir>");
        std::process::exit(2);
    });

    let donor_path = Path::new(&donor);
    let out_dir = PathBuf::from(out_dir);

    let bytes = fs::read(donor_path).unwrap_or_else(|e| {
        eprintln!("failed to read donor {donor}: {e}");
        std::process::exit(1);
    });
    fs::create_dir_all(&out_dir).unwrap_or_else(|e| {
        eprintln!("failed to create {}: {e}", out_dir.display());
        std::process::exit(1);
    });

    let boxes = scan_top_level_boxes(&bytes);
    println!("donor: {} bytes, {} top-level boxes", bytes.len(), boxes.len());
    for b in &boxes {
        println!("  {:>8}  @{:>10}  size={}", b.typ, b.offset, b.size);
    }

    // 1. Missing moov: truncate the donor right before the `moov` box so the
    //    parser sees ftyp + mdat but never finds moov.
    match boxes.iter().find(|b| b.typ == "moov") {
        Some(moov) => {
            let cut = moov.offset as usize;
            let name = "2026_01_01_000000_00_F.MP4";
            write_out(&out_dir, name, &bytes[..cut]);
            println!("→ {name} ({cut} bytes)  — expect: No index");
        }
        None => {
            eprintln!(
                "! donor has no moov box — skipping moov-missing fixture. \
                 Use a dashcam file that was written to completion."
            );
        }
    }

    // 2. Box overflow: pick the largest non-ftyp top-level box, keep its
    //    8-byte header (which still advertises the original size), and cut
    //    the file at offset + size/2. Parser reads the header, tries to
    //    read `size` bytes, hits EOF → "larger size than it".
    if let Some(big) = boxes
        .iter()
        .filter(|b| b.typ != "ftyp")
        .max_by_key(|b| b.size)
    {
        let cut = (big.offset + big.size / 2) as usize;
        let cut = cut.min(bytes.len());
        let name = "2026_01_01_000100_00_F.MP4";
        write_out(&out_dir, name, &bytes[..cut]);
        println!(
            "→ {name} ({cut} bytes, cut mid-`{}`) — expect: Corrupted",
            big.typ
        );
    }

    // 3. Zero-byte file with a dashcam filename.
    let name = "2026_01_01_000200_00_F.MP4";
    write_out(&out_dir, name, &[]);
    println!("→ {name} (0 bytes) — expect: MP4 error or Unreadable");

    // 4. A few KB of plain text with the dashcam filename + .MP4 extension.
    //    The mp4 crate reads garbage box headers and errors out.
    let garbage = b"This is not an MP4 file. \
        It is four kilobytes of plain text masquerading as one so the \
        scanner tries to parse it and fails.\n"
        .repeat(40);
    let name = "2026_01_01_000300_00_F.MP4";
    write_out(&out_dir, name, &garbage);
    println!("→ {name} ({} bytes, text) — expect: MP4 error", garbage.len());

    // 5. Valid donor content, but filename doesn't match any parser.
    let name = "random_garbage.mp4";
    write_out(&out_dir, name, &bytes);
    println!("→ {name} (donor copy) — expect: Bad name");

    println!("\ndone. Point Trip Viewer at {}", out_dir.display());
}

fn write_out(dir: &Path, name: &str, data: &[u8]) {
    let path = dir.join(name);
    if let Err(e) = fs::write(&path, data) {
        eprintln!("failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
}

struct BoxEntry {
    typ: String,
    offset: u64,
    size: u64,
}

/// Walk MP4 top-level boxes. Stops at the first malformed header or EOF.
/// Handles the 64-bit largesize extension (size == 1) and the
/// run-to-EOF marker (size == 0).
fn scan_top_level_boxes(bytes: &[u8]) -> Vec<BoxEntry> {
    let mut out = Vec::new();
    let len = bytes.len() as u64;
    let mut pos: u64 = 0;
    while pos + 8 <= len {
        let p = pos as usize;
        let declared = u32::from_be_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]);
        let typ = std::str::from_utf8(&bytes[p + 4..p + 8])
            .unwrap_or("????")
            .to_string();
        let size: u64 = match declared {
            1 => {
                if pos + 16 > len {
                    break;
                }
                u64::from_be_bytes([
                    bytes[p + 8],
                    bytes[p + 9],
                    bytes[p + 10],
                    bytes[p + 11],
                    bytes[p + 12],
                    bytes[p + 13],
                    bytes[p + 14],
                    bytes[p + 15],
                ])
            }
            0 => len - pos,
            n => u64::from(n),
        };
        if size < 8 || pos + size > len {
            break;
        }
        out.push(BoxEntry {
            typ,
            offset: pos,
            size,
        });
        pos += size;
    }
    out
}
