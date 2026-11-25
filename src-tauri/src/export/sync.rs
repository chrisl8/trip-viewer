//! Measures how far the cameras' recordings are out of step with each
//! other, using their sound. Every channel of a multi-camera recorder
//! picks up the same noises, so the delay at which one channel's audio
//! best matches the main channel's audio is the delay between the two
//! recordings. Used by the "Sync" button of the export dialog.
//!
//! The correlation is pure and Tauri-free so it can be unit-tested.

use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::export::plan::{ExportSegment, MAX_DELAY_MS};
use crate::timelapse::ffmpeg::ffmpeg_command;

/// Sample rate the audio is reduced to for matching. Plenty for finding a
/// delay to a fraction of a millisecond-scale; keeps the search cheap.
const RATE: usize = 2000;
/// How much audio is compared, from the start of the clip.
const WINDOW_S: f64 = 12.0;
/// Delays larger than this are not searched for (and are not accepted by
/// the export either).
const MAX_LAG_S: f64 = MAX_DELAY_MS / 1000.0 / 2.0;
/// Below this normalized match the result is not trusted.
const MIN_CONFIDENCE: f64 = 0.3;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectSyncArgs {
    pub segments: Vec<ExportSegment>,
    /// Start of the clip, seconds from the start of the trip.
    pub start_s: f64,
    pub main_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSync {
    pub label: String,
    /// How much to delay this channel to line it up with the main one.
    /// `None` when its sound could not be matched reliably.
    pub delay_ms: Option<f64>,
    /// 0..1, how closely the sounds matched at that delay.
    pub confidence: f64,
}

fn err(msg: impl Into<String>) -> AppError {
    AppError::Internal(msg.into())
}

/// Best `lag` (in samples, within ±`max_lag`) for which `main[n]` matches
/// `other[n - lag]`, with the normalized correlation at that lag. A
/// positive lag means the sound happens later in `main` than in `other`,
/// i.e. `other` has to be delayed by `lag` to line up.
pub fn best_lag(main: &[f32], other: &[f32], max_lag: usize) -> Option<(i64, f64)> {
    let n = main.len().min(other.len());
    if n <= 2 * max_lag + 1 {
        return None;
    }
    let mean = |v: &[f32]| v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(&main[..n]), mean(&other[..n]));
    let a: Vec<f64> = main[..n].iter().map(|&x| f64::from(x) - ma).collect();
    let b: Vec<f64> = other[..n].iter().map(|&x| f64::from(x) - mb).collect();
    let ea: f64 = a.iter().map(|x| x * x).sum();
    let eb: f64 = b.iter().map(|x| x * x).sum();
    if ea <= 0.0 || eb <= 0.0 {
        return None;
    }
    let norm = (ea * eb).sqrt();
    let max_lag = max_lag as i64;
    let mut best: Option<(i64, f64)> = None;
    for lag in -max_lag..=max_lag {
        // sum over n of a[n] * b[n - lag]
        let k = lag.unsigned_abs() as usize;
        let acc: f64 = if lag >= 0 {
            a[k..n].iter().zip(&b[..n - k]).map(|(x, y)| x * y).sum()
        } else {
            a[..n - k].iter().zip(&b[k..n]).map(|(x, y)| x * y).sum()
        };
        let c = acc / norm;
        if best.is_none_or(|(_, bc)| c > bc) {
            best = Some((lag, c));
        }
    }
    best
}

/// Mono 16-bit audio of `path` from `ss` for `len` seconds at `RATE`.
fn read_audio(ffmpeg_path: &str, path: &str, ss: f64, len: f64) -> Result<Vec<f32>, AppError> {
    let out = ffmpeg_command(ffmpeg_path)
        .args(["-v", "error", "-nostdin", "-ss"])
        .arg(format!("{ss:.3}"))
        .arg("-t")
        .arg(format!("{len:.3}"))
        .arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar"])
        .arg(RATE.to_string())
        .args(["-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| err(format!("could not run ffmpeg: {e}")))?;
    Ok(out
        .stdout
        .chunks(2)
        .filter(|b| b.len() == 2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])))
        .collect())
}

pub fn detect_sync(
    args: &DetectSyncArgs,
    ffmpeg_path: &str,
) -> Result<Vec<ChannelSync>, AppError> {
    if !args.start_s.is_finite() || args.start_s < 0.0 {
        return Err(err("invalid clip start"));
    }
    // The segment the clip starts in.
    let mut seg_start = 0.0;
    let mut found = None;
    for s in &args.segments {
        if !s.duration_s.is_finite() || s.duration_s <= 0.0 {
            return Err(err("a segment has no duration"));
        }
        if seg_start + s.duration_s > args.start_s {
            found = Some(s);
            break;
        }
        seg_start += s.duration_s;
    }
    let seg = found.ok_or_else(|| err("the clip starts after the end of the trip"))?;
    let window = WINDOW_S.min(seg.duration_s);
    let ss = (args.start_s - seg_start).min(seg.duration_s - window).max(0.0);

    let main_file = seg
        .files
        .iter()
        .find(|f| f.label == args.main_label)
        .ok_or_else(|| err(format!("{} has no file in this segment", args.main_label)))?;
    let main = read_audio(ffmpeg_path, &main_file.path, ss, window)?;
    let max_lag = (MAX_LAG_S * RATE as f64) as usize;
    if main.len() <= 2 * max_lag + 1 {
        return Err(err(format!("{} has no usable sound here", args.main_label)));
    }

    let mut out = Vec::new();
    for f in seg.files.iter().filter(|f| f.label != args.main_label) {
        let other = read_audio(ffmpeg_path, &f.path, ss, window)?;
        let (delay_ms, confidence) = match best_lag(&main, &other, max_lag) {
            Some((lag, c)) if c >= MIN_CONFIDENCE => {
                (Some((lag as f64 / RATE as f64 * 1000.0).round()), c)
            }
            Some((_, c)) => (None, c),
            None => (None, 0.0),
        };
        out.push(ChannelSync {
            label: f.label.clone(),
            delay_ms,
            confidence,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise-like signal.
    fn noise(n: usize) -> Vec<f32> {
        let mut x: u32 = 12345;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 16) as f32) - 32768.0
            })
            .collect()
    }

    #[test]
    fn a_channel_that_hears_things_earlier_has_to_be_delayed() {
        let src = noise(6000);
        // `other` has the same sound 120 samples earlier than `main`.
        let main = src[..5000].to_vec();
        let other = src[120..5120].to_vec();
        let (lag, c) = best_lag(&main, &other, 400).unwrap();
        assert_eq!(lag, 120);
        assert!(c > 0.9);
    }

    #[test]
    fn a_channel_that_hears_things_later_gets_a_negative_delay() {
        let src = noise(6000);
        let main = src[120..5120].to_vec();
        let other = src[..5000].to_vec();
        assert_eq!(best_lag(&main, &other, 400).unwrap().0, -120);
    }

    #[test]
    fn unrelated_sounds_do_not_match() {
        let a = noise(5000);
        let mut x: u32 = 99;
        let b: Vec<f32> = (0..5000)
            .map(|_| {
                x = x.wrapping_mul(22_695_477).wrapping_add(1);
                ((x >> 16) as f32) - 32768.0
            })
            .collect();
        let (_, c) = best_lag(&a, &b, 400).unwrap();
        assert!(c < MIN_CONFIDENCE);
    }

    #[test]
    fn silence_and_short_input_give_no_answer() {
        assert!(best_lag(&[0.0; 5000], &noise(5000), 400).is_none());
        assert!(best_lag(&noise(500), &noise(500), 400).is_none());
    }
}
