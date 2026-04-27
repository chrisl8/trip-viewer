//! Thin wrapper around `std::process::Command` for the ffmpeg binary.
//!
//! Three entry points:
//! - `probe_ffmpeg(path)` verifies the binary runs and reports whether
//!   `hevc_nvenc` is available. Called by the Test button in the
//!   settings dialog.
//! - `encode_trip_channel(...)` writes a concat list file, invokes
//!   ffmpeg to produce one timelapse output, and polls the cancel flag
//!   while the child runs. On cancel, the child is killed and the
//!   partial output is deleted.
//! - `generate_black_placeholder(...)` produces a short black-frame
//!   MP4 used to plug genuine sibling gaps so the concat demuxer sees
//!   a continuous stream and output stays in sync across channels.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::error::AppError;
use crate::timelapse::speed_curve;
use crate::timelapse::types::{Channel, EventWindow, FfmpegCapabilities, Tier};
use crate::timelapse::CancelFlag;

/// Run `ffmpeg -version` and `ffmpeg -encoders`, returning the parsed
/// capabilities. Returns an error if the binary can't be executed or
/// doesn't produce recognizable ffmpeg output.
pub fn probe_ffmpeg(path: &str) -> Result<FfmpegCapabilities, AppError> {
    let version_out = Command::new(path)
        .arg("-version")
        .output()
        .map_err(|e| AppError::Internal(format!("could not run ffmpeg at {path}: {e}")))?;
    if !version_out.status.success() {
        return Err(AppError::Internal(format!(
            "ffmpeg -version exited with status {}",
            version_out.status
        )));
    }
    let stdout = String::from_utf8_lossy(&version_out.stdout);
    let version_line = stdout
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if !version_line.starts_with("ffmpeg") {
        return Err(AppError::Internal(format!(
            "expected 'ffmpeg version ...' but got: {version_line}"
        )));
    }

    // `-encoders` lists everything compiled in; look for hevc_nvenc.
    let encoders_out = Command::new(path)
        .arg("-hide_banner")
        .arg("-encoders")
        .output()
        .map_err(|e| AppError::Internal(format!("could not list ffmpeg encoders: {e}")))?;
    let encoders_text = String::from_utf8_lossy(&encoders_out.stdout);
    let nvenc_hevc = encoders_text.contains("hevc_nvenc");

    Ok(FfmpegCapabilities {
        version: version_line,
        nvenc_hevc,
    })
}

/// What encoder the caller picked for this invocation. Decided once at
/// job-start time from the cached capabilities so a whole batch uses a
/// consistent codec.
#[derive(Debug, Clone, Copy)]
pub enum Encoder {
    HevcNvenc,
    LibX265,
}

impl Encoder {
    pub fn as_str(&self) -> &'static str {
        match self {
            Encoder::HevcNvenc => "hevc_nvenc",
            Encoder::LibX265 => "libx265",
        }
    }

    /// The filter name used for scaling in the filter graph. NVENC uses
    /// `scale_cuda` so frames stay on the GPU end-to-end — without this
    /// the filter chain downloads every frame to CPU for `scale`, then
    /// re-uploads to GPU for NVENC, which starves the encoder and pegs
    /// one CPU core instead of using the GPU.
    pub fn scale_filter(&self) -> &'static str {
        match self {
            Encoder::HevcNvenc => "scale_cuda",
            Encoder::LibX265 => "scale",
        }
    }

    /// Whether this encoder wants `-hwaccel cuda -hwaccel_output_format cuda`
    /// on the input so NVDEC handles decode and frames land in GPU memory
    /// ready for `scale_cuda` → NVENC.
    pub fn needs_cuda_hwaccel(&self) -> bool {
        matches!(self, Encoder::HevcNvenc)
    }

    pub fn pick(caps: &FfmpegCapabilities) -> Self {
        if caps.nvenc_hevc {
            Encoder::HevcNvenc
        } else {
            Encoder::LibX265
        }
    }
}

/// Arguments for one per-channel encode. `source_paths` are the
/// ordered segment files for this trip+channel. `output_path` is the
/// final .mp4 location; the function overwrites anything already there.
///
/// `windows` + `total_duration_s` feed `speed_curve::compose_filter`
/// to produce the `filter_complex` body. For fixed tiers this is a
/// one-stage passthrough; for variable tiers it's the alternating
/// base/event-rate concat. The three channels of a given trip-tier
/// share identical `(windows, total_duration_s)` so they stay synced.
pub struct EncodeArgs<'a> {
    pub ffmpeg_path: &'a str,
    pub source_paths: &'a [String],
    pub output_path: &'a Path,
    pub tier: Tier,
    #[allow(dead_code)] // referenced by future log lines and metrics
    pub channel: Channel,
    pub encoder: Encoder,
    pub windows: &'a [EventWindow],
    pub total_duration_s: f64,
}

/// Encode one (trip, tier, channel) output. Blocks until ffmpeg exits,
/// polling `cancel` every 500ms; if cancelled, kills the child and
/// deletes the partial output before returning.
///
/// Writes a temp concat-list file next to the output; removes it on
/// success or failure. Returns `Ok(output_path)` on success.
pub fn encode_trip_channel(
    args: &EncodeArgs<'_>,
    cancel: &CancelFlag,
) -> Result<PathBuf, AppError> {
    if args.source_paths.is_empty() {
        return Err(AppError::Internal("no source segments for trip".into()));
    }

    if let Some(parent) = args.output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Remove any stale output file from a previous failed attempt so
    // ffmpeg doesn't inherit a partial moov atom or confuse muxers.
    if args.output_path.exists() {
        let _ = fs::remove_file(args.output_path);
    }

    let concat_path = concat_list_path(args.output_path);
    write_concat_file(&concat_path, args.source_paths)?;

    let filter = speed_curve::compose_filter(
        args.windows,
        args.tier,
        args.total_duration_s,
        args.encoder.scale_filter(),
    );

    let mut cmd = Command::new(args.ffmpeg_path);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error");

    // Hardware-accelerated decode path. Critical: without this, NVENC
    // ends up starved because frames are decoded + filtered on the CPU
    // and uploaded per-frame to the GPU for encode. With this, NVDEC
    // decodes to GPU memory, `scale_cuda` scales in place, and NVENC
    // reads directly from GPU memory — zero host↔device copies.
    //
    // `-hwaccel_output_format cuda` is the load-bearing bit: without it
    // ffmpeg downloads frames back to system memory after decode.
    if args.encoder.needs_cuda_hwaccel() {
        cmd.arg("-hwaccel")
            .arg("cuda")
            .arg("-hwaccel_output_format")
            .arg("cuda");
    }

    cmd.arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&concat_path)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[out]")
        .arg("-an");

    match args.encoder {
        Encoder::HevcNvenc => {
            cmd.arg("-c:v")
                .arg("hevc_nvenc")
                .arg("-preset")
                .arg("p5")
                .arg("-cq")
                .arg("26");
        }
        Encoder::LibX265 => {
            cmd.arg("-c:v")
                .arg("libx265")
                .arg("-crf")
                .arg("26")
                .arg("-preset")
                .arg("medium")
                .arg("-x265-params")
                .arg("log-level=error");
        }
    }

    cmd.arg(args.output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Internal(format!("failed to spawn ffmpeg: {e}")))?;

    // Poll for exit or cancel. 500ms is a compromise between cancel
    // responsiveness and CPU wakeups — the worker-level progress
    // events already throttle at 250ms, so sub-second cancel is
    // plenty responsive for the user.
    let cancelled = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        match child.try_wait() {
            Ok(Some(_status)) => break false,
            Ok(None) => thread::sleep(Duration::from_millis(500)),
            Err(e) => {
                let _ = fs::remove_file(&concat_path);
                return Err(AppError::Internal(format!(
                    "error waiting on ffmpeg: {e}"
                )));
            }
        }
    };

    let output = child
        .wait_with_output()
        .map_err(|e| AppError::Internal(format!("ffmpeg wait_with_output failed: {e}")))?;
    let _ = fs::remove_file(&concat_path);

    if cancelled {
        // Partial output is garbage (no moov atom) — nuke it.
        if args.output_path.exists() {
            let _ = fs::remove_file(args.output_path);
        }
        return Err(AppError::Internal("cancelled".into()));
    }

    if !output.status.success() {
        if args.output_path.exists() {
            let _ = fs::remove_file(args.output_path);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = tail_lines(&stderr, 8);
        return Err(AppError::Internal(format!(
            "ffmpeg exited with {}: {tail}",
            output.status
        )));
    }

    Ok(args.output_path.to_path_buf())
}

fn concat_list_path(output_path: &Path) -> PathBuf {
    let mut p = output_path.to_path_buf();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "trip".to_string());
    p.set_file_name(format!(".{stem}.concat.txt"));
    p
}

/// Write the ffmpeg concat demuxer list. Each line is
/// `file 'escaped/path'`. The demuxer's quoting is primitive: single
/// quotes inside paths are escaped by closing, adding a literal, and
/// reopening.
fn write_concat_file(path: &Path, sources: &[String]) -> Result<(), AppError> {
    let mut f = fs::File::create(path)?;
    for src in sources {
        let escaped = src.replace('\'', "'\\''");
        writeln!(f, "file '{escaped}'")?;
    }
    Ok(())
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Produce a short black-frame MP4 at the target `output` path with
/// matching codec/resolution/framerate so it can be fed alongside the
/// real channel files through the ffmpeg concat demuxer.
///
/// The concat demuxer requires all inputs to share codec + pixel
/// format + resolution + framerate. Callers probe a reference real
/// sibling via `metadata::mp4_probe::probe` and hand those params in
/// here so the placeholder sits flush with its neighbors in the list.
pub fn generate_black_placeholder(
    ffmpeg_path: &str,
    output: &Path,
    width: u32,
    height: u32,
    fps: u32,
    duration_s: f64,
    encoder: Encoder,
) -> Result<(), AppError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    if output.exists() {
        let _ = fs::remove_file(output);
    }

    let color_arg = format!(
        "color=black:size={}x{}:rate={}:duration={:.3}",
        width, height, fps, duration_s,
    );

    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(&color_arg)
        .arg("-an");

    match encoder {
        Encoder::HevcNvenc => {
            cmd.arg("-c:v")
                .arg("hevc_nvenc")
                .arg("-preset")
                .arg("p5")
                .arg("-cq")
                .arg("26");
        }
        Encoder::LibX265 => {
            cmd.arg("-c:v")
                .arg("libx265")
                .arg("-crf")
                .arg("26")
                .arg("-preset")
                .arg("medium")
                .arg("-x265-params")
                .arg("log-level=error");
        }
    }

    cmd.arg(output).stdout(Stdio::null()).stderr(Stdio::piped());

    let out = cmd
        .output()
        .map_err(|e| AppError::Internal(format!("spawn ffmpeg for placeholder failed: {e}")))?;
    if !out.status.success() {
        if output.exists() {
            let _ = fs::remove_file(output);
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Internal(format!(
            "ffmpeg placeholder exited {}: {}",
            out.status,
            tail_lines(&stderr, 8),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn concat_list_path_sits_beside_output() {
        let out = PathBuf::from("C:/Archive/Timelapses/abc123_8x_F.mp4");
        let concat = concat_list_path(&out);
        assert_eq!(
            concat,
            PathBuf::from("C:/Archive/Timelapses/.abc123_8x_F.concat.txt")
        );
    }

    #[test]
    fn write_concat_file_escapes_single_quotes() {
        let path = temp_dir().join("tripviewer-test-concat.txt");
        let _ = fs::remove_file(&path);
        let sources = vec![
            "C:/vids/a.mp4".to_string(),
            "C:/dude's stuff/b.mp4".to_string(),
        ];
        write_concat_file(&path, &sources).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("file 'C:/vids/a.mp4'"));
        assert!(contents.contains("file 'C:/dude'\\''s stuff/b.mp4'"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn encoder_picks_nvenc_when_available() {
        let caps = FfmpegCapabilities {
            version: "ffmpeg version 7.0".into(),
            nvenc_hevc: true,
        };
        assert!(matches!(Encoder::pick(&caps), Encoder::HevcNvenc));
        let caps = FfmpegCapabilities {
            version: "ffmpeg version 7.0".into(),
            nvenc_hevc: false,
        };
        assert!(matches!(Encoder::pick(&caps), Encoder::LibX265));
    }

    /// Integration-lite: runs `generate_black_placeholder` against a
    /// real ffmpeg binary and verifies the output exists and parses
    /// as an MP4 with the requested duration. Skipped unless the
    /// `TRIPVIEWER_FFMPEG` env var points at a working ffmpeg — CI
    /// doesn't need to bundle ffmpeg just to run unit tests.
    ///
    /// Run locally with:
    ///   $ TRIPVIEWER_FFMPEG=/path/to/ffmpeg cargo test --manifest-path src-tauri/Cargo.toml \
    ///         --lib timelapse::ffmpeg::tests::placeholder_produces_valid_mp4_with_requested_duration
    #[test]
    fn placeholder_produces_valid_mp4_with_requested_duration() {
        let Ok(ffmpeg) = std::env::var("TRIPVIEWER_FFMPEG") else {
            eprintln!("[skip] TRIPVIEWER_FFMPEG not set");
            return;
        };

        let out = temp_dir().join(format!(
            "tripviewer-placeholder-test-{}.mp4",
            std::process::id()
        ));
        let _ = fs::remove_file(&out);

        // Use libx265 in the test so GPUs aren't required on dev boxes.
        // The branch tested here is the same `generate_black_placeholder`
        // entry point the NVENC path uses; only the encoder args differ.
        generate_black_placeholder(
            &ffmpeg,
            &out,
            640,
            360,
            30,
            2.0,
            Encoder::LibX265,
        )
        .expect("placeholder generation should succeed with a real ffmpeg");

        assert!(out.exists(), "placeholder file wasn't written");
        let size = fs::metadata(&out).unwrap().len();
        assert!(size > 0, "placeholder file is empty");

        // Probe the output with the project's existing mp4 parser and
        // check the reported duration lands within 0.1 s of requested.
        let meta = crate::metadata::mp4_probe::probe(&out)
            .expect("generated placeholder should parse as MP4");
        assert!(
            (meta.duration_s - 2.0).abs() < 0.1,
            "expected ~2.0 s, got {}",
            meta.duration_s
        );
        assert_eq!(meta.width, 640);
        assert_eq!(meta.height, 360);

        let _ = fs::remove_file(&out);
    }
}
