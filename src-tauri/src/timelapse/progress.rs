//! FFmpeg's machine-readable progress; independent of the stderr diagnostic tail.
use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::time::Instant;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodingProgress {
    pub phase: &'static str,
    pub encoder: &'static str,
    pub processed_seconds: f64,
    pub total_seconds: f64,
    pub elapsed_seconds: f64,
}

pub type ProgressCallback = Arc<dyn Fn(EncodingProgress) + Send + Sync>;

#[derive(Clone)]
pub struct Reporter {
    callback: ProgressCallback,
    phase: &'static str,
    encoder: &'static str,
    total: f64,
    offset: f64,
    started: Instant,
}

impl Reporter {
    pub fn new(callback: ProgressCallback, phase: &'static str, encoder: &'static str, total: f64) -> Self {
        Self { callback, phase, encoder, total, offset: 0.0, started: Instant::now() }
    }

    pub fn offset(&self, offset: f64) -> Self { Self { offset, ..self.clone() } }

    pub fn emit(&self, seconds: f64) {
        (self.callback)(EncodingProgress {
            phase: self.phase, encoder: self.encoder,
            processed_seconds: (self.offset + seconds).clamp(0.0, self.total.max(0.0)),
            total_seconds: self.total,
            elapsed_seconds: self.started.elapsed().as_secs_f64(),
        });
    }

    pub fn read(&self, reader: impl Read) {
        let mut seconds = 0.0;
        self.emit(0.0);
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                // FFmpeg's legacy out_time_ms key also contains microseconds.
                if let Ok(micros) = value.parse::<i64>() {
                    seconds = (micros.max(0) as f64) / 1_000_000.0;
                }
            } else if line.starts_with("progress=") {
                self.emit(seconds);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn parses_microseconds_and_legacy_units_without_accumulating_frames() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let report = Reporter::new(Arc::new(move |p| sink.lock().unwrap().push(p)), "encoding", "libx265", 10.0);
        report.offset(2.0).read(&b"frame=30\nout_time_us=1500000\nprogress=continue\nout_time_ms=999999999\nprogress=end\n"[..]);
        let events = events.lock().unwrap();
        assert_eq!(events[1].processed_seconds, 3.5);
        assert_eq!(events[2].processed_seconds, 10.0);
        assert_eq!(events[1].phase, "encoding");
    }

    #[test]
    fn malformed_or_negative_times_keep_a_safe_heartbeat() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let report = Reporter::new(Arc::new(move |p| sink.lock().unwrap().push(p)), "preparing", "libx265", 5.0);
        report.read(&b"out_time_us=N/A\nprogress=continue\nout_time_us=-500\nprogress=end\n"[..]);
        assert!(events.lock().unwrap().iter().all(|p| p.processed_seconds == 0.0));
    }
}
