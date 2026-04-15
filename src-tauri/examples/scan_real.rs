use std::path::Path;
use std::time::Instant;
use tripviewer_lib::scan::scan_folder_sync;

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "E:\\Wolfbox Dashcam\\Videos".to_string());
    let path = Path::new(&root);

    println!("scanning: {}", path.display());
    let start = Instant::now();
    let result = match scan_folder_sync(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        }
    };
    let elapsed = start.elapsed();

    let segments: usize = result.trips.iter().map(|t| t.segments.len()).sum();
    let normal = result
        .trips
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter(|s| !s.is_event)
        .count();
    let events = result
        .trips
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter(|s| s.is_event)
        .count();

    println!();
    println!("  elapsed:    {elapsed:.2?}");
    println!("  trips:      {}", result.trips.len());
    println!("  segments:   {segments}  ({normal} normal + {events} event)");
    println!("  unmatched:  {}", result.unmatched.len());
    println!("  errors:     {}", result.errors.len());

    if !result.errors.is_empty() {
        println!();
        println!("first 5 errors:");
        for e in result.errors.iter().take(5) {
            println!("  {} — {}", e.path, e.reason);
        }
    }

    if !result.unmatched.is_empty() {
        println!();
        println!("first 5 unmatched:");
        for u in result.unmatched.iter().take(5) {
            println!("  {u}");
        }
    }

    if let Some(trip) = result.trips.first() {
        println!();
        println!("first trip:");
        println!("  start: {}", trip.start_time);
        println!("  end:   {}", trip.end_time);
        println!("  segments: {}", trip.segments.len());
        if let Some(seg) = trip.segments.first() {
            println!(
                "  first segment: {} ({:.1}s) — {} channels",
                seg.start_time,
                seg.duration_s,
                seg.channels.len()
            );
            for ch in &seg.channels {
                println!(
                    "    {}: {}x{} codec={:?} gpmd={}",
                    ch.label,
                    ch.width.unwrap_or(0),
                    ch.height.unwrap_or(0),
                    ch.codec,
                    ch.has_gpmd_track
                );
            }
        }
    }
}
