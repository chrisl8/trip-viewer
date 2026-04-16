use std::fs::File;
use std::io::BufReader;
use std::path::Path;

fn main() {
    let path_str = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "E:\\Wolfbox Dashcam\\Videos\\2026_03_23_094634_00_F.MP4".to_string());
    let path = Path::new(&path_str);

    println!("=== GPS debug for: {path_str}");

    let file = File::open(path).expect("open file");
    let size = file.metadata().unwrap().len();
    let reader = BufReader::new(file);
    let mut mp4 = mp4::Mp4Reader::read_header(reader, size).expect("mp4 header");

    println!("\ntracks:");
    for (id, track) in mp4.tracks() {
        let ht = track.trak.mdia.hdlr.handler_type;
        let ht_str = String::from_utf8_lossy(&ht.value);
        let samples = track.sample_count();
        let dur = track.duration().as_secs_f64();
        println!(
            "  track {id}: handler={ht_str:?} type={:?} samples={samples} duration={dur:.1}s",
            track.track_type()
        );
    }

    // Find metadata track
    let meta_id = mp4.tracks().iter().find_map(|(id, track)| {
        let ht = track.trak.mdia.hdlr.handler_type.value;
        if ht != *b"vide" && ht != *b"soun" && ht != *b"sbtl" && ht != [0, 0, 0, 0] {
            Some(*id)
        } else {
            None
        }
    });

    let meta_id = match meta_id {
        Some(id) => {
            println!("\nusing metadata track {id}");
            id
        }
        None => {
            println!("\nno metadata track found!");
            return;
        }
    };

    let sample_count = mp4.tracks()[&meta_id].sample_count();
    let timescale = mp4.tracks()[&meta_id].timescale();
    println!("  sample_count={sample_count}  timescale={timescale}");

    // Read first 3 samples and dump GPMF keys
    for sid in 1..=sample_count.min(3) {
        match mp4.read_sample(meta_id, sid) {
            Ok(Some(sample)) => {
                println!(
                    "\nsample {sid}: start_time={} duration={} bytes={}",
                    sample.start_time,
                    sample.duration,
                    sample.bytes.len()
                );
                dump_gpmf_keys(&sample.bytes, 0);
            }
            Ok(None) => println!("\nsample {sid}: None"),
            Err(e) => println!("\nsample {sid}: error: {e}"),
        }
    }

    // Now try actual extraction
    println!("\n=== Running extract...");
    match tripviewer_lib::gps::shenshu::extract(path) {
        Ok(pts) => {
            println!("{} GPS points", pts.len());
            for p in pts.iter().take(5) {
                println!(
                    "  t={:.2}s lat={:.6} lon={:.6} speed={:.1}m/s alt={:.1}m",
                    p.t_offset_s, p.lat, p.lon, p.speed_mps, p.altitude_m
                );
            }
        }
        Err(e) => println!("extract error: {e}"),
    }
}

fn dump_gpmf_keys(data: &[u8], depth: usize) {
    let indent = "  ".repeat(depth + 2);
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let key = &data[pos..pos + 4];
        let key_str = String::from_utf8_lossy(key);
        let type_char = data[pos + 4];
        let struct_size = data[pos + 5] as u16;
        let repeat = u16::from_be_bytes([data[pos + 6], data[pos + 7]]);
        let payload_len = struct_size as usize * repeat as usize;
        let aligned_len = (payload_len + 3) & !3;
        pos += 8;

        if pos + payload_len > data.len() {
            println!("{indent}[{key_str}] type=0x{type_char:02x}({}) size={struct_size} repeat={repeat} TRUNCATED (need {payload_len}, have {})",
                type_char as char, data.len() - pos);
            break;
        }

        let payload = &data[pos..pos + payload_len];

        if type_char == 0 {
            println!("{indent}[{key_str}] CONTAINER  children={payload_len}B");
            dump_gpmf_keys(payload, depth + 1);
        } else {
            let type_ch = if type_char.is_ascii_graphic() {
                type_char as char
            } else {
                '?'
            };
            let preview = if payload_len <= 40 && type_char == b'c' {
                format!(" = {:?}", String::from_utf8_lossy(payload))
            } else if type_char == b'l' && payload_len >= 4 {
                let v = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                format!(" first_val={v}")
            } else {
                String::new()
            };
            println!("{indent}[{key_str}] type='{type_ch}' size={struct_size} repeat={repeat} payload={payload_len}B{preview}");
        }

        pos += aligned_len;
    }
}
