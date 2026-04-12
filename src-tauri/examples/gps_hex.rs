use std::fs::File;
use std::io::BufReader;
use std::path::Path;

fn main() {
    let path_str = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "E:\\Wolfbox Dashcam\\Videos\\2026_04_10_160549_00_F.MP4".to_string());
    let path = Path::new(&path_str);

    let file = File::open(path).expect("open");
    let size = file.metadata().unwrap().len();
    let reader = BufReader::new(file);
    let mut mp4 = mp4::Mp4Reader::read_header(reader, size).expect("mp4");

    let meta_id = mp4.tracks().iter().find_map(|(id, track)| {
        let ht = track.trak.mdia.hdlr.handler_type.value;
        if ht == *b"meta" { Some(*id) } else { None }
    }).expect("no metadata track");

    let sample_count = mp4.tracks()[&meta_id].sample_count();
    let timescale = mp4.tracks()[&meta_id].timescale();
    println!("file: {path_str}");
    println!("track {meta_id}: {sample_count} samples, timescale={timescale}\n");

    // Dump ALL i32 fields at 4-byte intervals for first sample
    if let Ok(Some(s)) = mp4.read_sample(meta_id, 1) {
        let d = &s.bytes[..];
        println!("sample 1: {} bytes", d.len());
        println!("all LE i32 values:");
        for off in (0..d.len().min(160)).step_by(4) {
            let v = le_i32(d, off);
            if v != 0 {
                println!("  0x{off:02x}: {v:>12}  (0x{:08x})", v as u32);
            }
        }
    }

    // Now show specific fields for samples 1..50, including several candidate speed/alt offsets
    println!("\n{:>4} {:>7}  {:>11} {:>12}  0x08  0x48  0x58  0x0A(u16)  0x0C(u16)",
        "sid", "t_s", "lat", "lon");

    for sid in 1..=sample_count.min(50) {
        let sample = match mp4.read_sample(meta_id, sid) {
            Ok(Some(s)) => s,
            _ => continue,
        };
        let d = &sample.bytes[..];
        let t_s = sample.start_time as f64 / timescale as f64;
        if d.len() < 0x78 { continue; }

        let lat = le_i32(d, 0x28) as f64 / 1e7;
        let lon = le_i32(d, 0x38) as f64 / 1e7;
        let v08 = le_i32(d, 0x08);
        let v48 = le_i32(d, 0x48);
        let v58 = le_i32(d, 0x58);
        let u16_0a = u16::from_le_bytes([d[0x0A], d[0x0B]]);
        let u16_0c = u16::from_le_bytes([d[0x0C], d[0x0D]]);

        if sid <= 10 || sid % 10 == 0 {
            println!("{sid:4} {t_s:7.2}  {lat:11.7} {lon:12.7}  {v08:5} {v48:5} {v58:5}  {u16_0a:10}  {u16_0c:9}");
        }
    }
}

fn le_i32(d: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([d[off], d[off+1], d[off+2], d[off+3]])
}
