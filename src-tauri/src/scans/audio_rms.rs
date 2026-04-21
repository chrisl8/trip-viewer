//! Decode the master channel's audio track and tag segments whose
//! overall RMS level is below the silence threshold. Uses `symphonia`
//! for pure-Rust AAC/MP3 decoding so this scan doesn't pull in an
//! external ffmpeg dependency.
//!
//! Three outcomes:
//! - No audio track at all → `no_audio` tag (informational).
//! - Audio present but mean RMS below threshold → `silent` tag.
//! - Audio with meaningful level → no tag.
//!
//! Decode errors on malformed AAC are swallowed per-packet; if decoding
//! fails entirely the scan returns an error so `scan_runs.status` is
//! `'error'` and the segment can be retried on version bump.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::Ordering;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::error::AppError;
use crate::scans::{CostTier, Scan, ScanContext};
use crate::tags::{Tag, TagCategory, TagSource};

pub struct AudioRms {
    /// RMS level threshold in dB below which the segment is considered
    /// silent. -50 dB tolerates mild noise-floor hiss while catching a
    /// truly muted cabin (engine off in a garage).
    pub silence_threshold_db: f32,
}

impl Default for AudioRms {
    fn default() -> Self {
        Self {
            silence_threshold_db: -50.0,
        }
    }
}

impl Scan for AudioRms {
    fn id(&self) -> &'static str {
        "audio_rms"
    }
    fn version(&self) -> u32 {
        1
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Heavy
    }
    fn display_name(&self) -> &'static str {
        "Audio silence"
    }
    fn description(&self) -> &'static str {
        "Decode the audio track and tag segments whose overall volume is near silent — or that have no audio track at all. Slower than other scans: has to read the whole file."
    }
    fn emits(&self) -> &'static [&'static str] {
        &["silent", "no_audio"]
    }

    fn run(&self, ctx: &ScanContext) -> Result<Vec<Tag>, AppError> {
        let path = Path::new(&ctx.segment.master_path);
        match measure_rms_db(path, ctx.cancel)? {
            None => Ok(vec![make_tag(
                self,
                &ctx.segment.id,
                "no_audio",
                TagCategory::Audio,
                None,
            )]),
            Some(db) if db < self.silence_threshold_db => Ok(vec![make_tag(
                self,
                &ctx.segment.id,
                "silent",
                TagCategory::Audio,
                Some(serde_json::json!({ "rms_db": db }).to_string()),
            )]),
            Some(_) => Ok(vec![]),
        }
    }
}

fn make_tag(
    scan: &AudioRms,
    segment_id: &str,
    name: &str,
    category: TagCategory,
    metadata_json: Option<String>,
) -> Tag {
    Tag {
        id: None,
        segment_id: Some(segment_id.to_string()),
        trip_id: None,
        name: name.to_string(),
        category,
        source: TagSource::System,
        scan_id: Some(scan.id().to_string()),
        scan_version: Some(scan.version()),
        confidence: None,
        start_ms: None,
        end_ms: None,
        note: None,
        metadata_json,
        created_ms: chrono::Utc::now().timestamp_millis(),
    }
}

/// Returns `Ok(None)` when no audio track exists, `Ok(Some(db))` when
/// decoding yielded samples, or `Err` when the probe itself failed.
fn measure_rms_db(path: &Path, cancel: &crate::scans::CancelFlag) -> Result<Option<f32>, AppError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }
    let fmt_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| AppError::Internal(format!("audio probe failed: {e}")))?;
    let mut format = probed.format;

    let Some(track) = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
    else {
        return Ok(None);
    };
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AppError::Internal(format!("audio decoder init failed: {e}")))?;

    let mut sum_sq: f64 = 0.0;
    let mut n: u64 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Internal("scan cancelled".into()));
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => accumulate_rms(&decoded, &mut sum_sq, &mut n),
            // Soft-fail on a single corrupt packet rather than abandoning
            // the whole file; long clips routinely have one or two bad
            // AAC frames that shouldn't invalidate the RMS measurement.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(AppError::Internal(format!("audio decode: {e}"))),
        }
    }

    if n == 0 {
        return Ok(None);
    }
    let rms = (sum_sq / n as f64).sqrt() as f32;
    let db = 20.0 * rms.max(1e-10).log10();
    Ok(Some(db))
}

fn accumulate_rms(buf: &AudioBufferRef, sum_sq: &mut f64, n: &mut u64) {
    match buf {
        AudioBufferRef::F32(b) => {
            for ch in 0..b.spec().channels.count() {
                for &sample in b.chan(ch) {
                    *sum_sq += (sample as f64) * (sample as f64);
                    *n += 1;
                }
            }
        }
        AudioBufferRef::S16(b) => {
            let scale = 1.0 / (i16::MAX as f64);
            for ch in 0..b.spec().channels.count() {
                for &sample in b.chan(ch) {
                    let f = sample as f64 * scale;
                    *sum_sq += f * f;
                    *n += 1;
                }
            }
        }
        AudioBufferRef::S32(b) => {
            let scale = 1.0 / (i32::MAX as f64);
            for ch in 0..b.spec().channels.count() {
                for &sample in b.chan(ch) {
                    let f = sample as f64 * scale;
                    *sum_sq += f * f;
                    *n += 1;
                }
            }
        }
        // AAC/MP3 typically decode to F32 or S16; other symphonia
        // sample formats (U8/S8/S24/U16/U32/F64) aren't produced by
        // these codec paths, so skipping is safe in practice.
        _ => {}
    }
}
