//! Thin wrapper around `std::process::Command` for the ffmpeg binary.
//!
//! Three entry points:
//! - `probe_ffmpeg(path)` verifies the binary runs and reports whether
//!   `hevc_nvenc` is available. Called by the Test button in the
//!   settings dialog.
//! - `encode_trip_channel(...)` invokes ffmpeg with each segment as a
//!   separate `-i` input fed through the concat *filter* (not the
//!   concat demuxer), polling the cancel flag while the child runs.
//!   On cancel, the child is killed and the partial output deleted.
//!   The concat filter is load-bearing on the CUDA path: NVDEC +
//!   scale_cuda + concat *demuxer* fails reliably with
//!   "Error reinitializing filters! ... -40 (Function not implemented)"
//!   when the segment changes mid-stream, even when the inputs are
//!   parameter-uniform. The concat filter normalizes streams across
//!   inputs in the filter graph and survives the same boundaries.
//! - `generate_black_placeholder(...)` produces a short black-frame
//!   MP4 used to plug genuine sibling gaps so each per-channel concat
//!   has a uniform-length stream and the channels stay in sync.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::error::AppError;
use crate::timelapse::speed_curve;
use crate::timelapse::types::{Channel, EventWindow, FfmpegCapabilities, Tier};
use crate::timelapse::CancelFlag;

// On Windows, a GUI-subsystem process (the installed build sets
// `windows_subsystem = "windows"`) that spawns a console child via
// `Command::new` gets a fresh console window unless `CREATE_NO_WINDOW`
// is set on the creation flags. Route every ffmpeg invocation in the
// crate through this helper so the installed build runs silently.
#[cfg(windows)]
pub(crate) fn ffmpeg_command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
pub(crate) fn ffmpeg_command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    Command::new(program)
}

/// Run `ffmpeg -version` and `ffmpeg -encoders`, returning the parsed
/// capabilities. Returns an error if the binary can't be executed or
/// doesn't produce recognizable ffmpeg output.
pub fn probe_ffmpeg(path: &str) -> Result<FfmpegCapabilities, AppError> {
    let version_out = ffmpeg_command(path)
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
    let encoders_out = ffmpeg_command(path)
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
/// deletes the partial output before returning. Returns
/// `Ok(output_path)` on success.
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

    let n_inputs = args.source_paths.len();
    // For 2+ inputs, prepend a concat filter that joins them into one
    // stream (`[vcat]`) which the existing speed-curve filter then
    // consumes. Single-input case skips the concat prefix and reads
    // directly from input 0's video stream — same shape as before the
    // refactor.
    //
    // We select the *first* video stream explicitly via `[N:v:0]`
    // rather than `[N:v]`. The concat filter wants exactly v=1 per
    // input; `[N:v]` matches all video streams, so a (rare) multi-
    // video-stream file would feed several into a slot expecting one
    // and the filter graph would fail to instantiate. Audio streams
    // are not referenced anywhere — combined with `concat=...:a=0`
    // and the output-side `-an` below, audio is dropped at every
    // level, so audio-bearing dashcam files (e.g. Wolf Box's in-cabin
    // mic) feed through without trouble.
    let head_label = if n_inputs == 1 { "0:v:0" } else { "vcat" };
    let speed_filter = speed_curve::compose_filter(
        args.windows,
        args.tier,
        args.total_duration_s,
        args.encoder.scale_filter(),
        head_label,
    );
    let filter = if n_inputs == 1 {
        speed_filter
    } else {
        let mut prefix = String::new();
        for i in 0..n_inputs {
            prefix.push_str(&format!("[{i}:v:0]"));
        }
        prefix.push_str(&format!("concat=n={n_inputs}:v=1:a=0[vcat];"));
        prefix.push_str(&speed_filter);
        prefix
    };

    let mut cmd = ffmpeg_command(args.ffmpeg_path);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error");

    // Per-input hwaccel flags. NVENC pipeline keeps NVDEC → scale_cuda
    // → NVENC entirely on the GPU; without this each frame round-trips
    // through host memory and one CPU core caps throughput.
    //
    // The hwaccel options bind to the *next* `-i` only — so this loop
    // re-emits them in front of every input rather than once at the
    // top of the command line.
    for src in args.source_paths {
        if args.encoder.needs_cuda_hwaccel() {
            cmd.arg("-hwaccel")
                .arg("cuda")
                .arg("-hwaccel_output_format")
                .arg("cuda");
        }
        cmd.arg("-i").arg(src);
    }

    cmd.arg("-filter_complex")
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
                return Err(AppError::Internal(format!(
                    "error waiting on ffmpeg: {e}"
                )));
            }
        }
    };

    let output = child
        .wait_with_output()
        .map_err(|e| AppError::Internal(format!("ffmpeg wait_with_output failed: {e}")))?;

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

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Pixel format and color tagging probed off a reference real-channel
/// file so we can bake the same values into the black placeholders.
///
/// Why this matters: ffmpeg initializes the filter graph from the first
/// concat segment's frame parameters and can't always reinit when the
/// next segment differs. A `lavfi color=black` placeholder defaults to
/// `yuv420p` (limited range, no color tags); Wolf Box footage is
/// `yuvj420p` (full range) with explicit BT.709 (or BT.601 on the
/// interior cam) color metadata. When the placeholder leads the concat,
/// the auto_scaler set up for `yuv420p` blows up with -40 ENOSYS the
/// instant a `yuvj420p` real frame arrives. Matching these fields
/// keeps the decoded streams parameter-identical.
#[derive(Debug, Clone)]
pub struct ColorMetadata {
    pub pix_fmt: String,
    pub color_range: Option<String>,
    pub color_primaries: Option<String>,
    pub color_trc: Option<String>,
    pub color_space: Option<String>,
}

impl ColorMetadata {
    /// Sane Wolf Box-shaped defaults used when probing fails or the
    /// stderr line doesn't match the parser. Better to encode a
    /// placeholder with the most common tags than to bail the whole
    /// trip — if the assumption is wrong, the auto_scaler reinit will
    /// still surface as a job failure rather than silent corruption.
    fn fallback() -> Self {
        Self {
            pix_fmt: "yuvj420p".to_string(),
            color_range: Some("pc".to_string()),
            color_primaries: Some("bt709".to_string()),
            color_trc: Some("bt709".to_string()),
            color_space: Some("bt709".to_string()),
        }
    }
}

/// Probe a real reference file for pix_fmt + color tags by invoking
/// ffmpeg in a no-op mode (`-i file -t 0 -f null -`). ffmpeg writes
/// stream info to stderr before bailing on the empty output, which we
/// parse. Avoids a separate ffprobe dependency (DESIGN.md locks ffprobe
/// out of the bundle); the user's existing ffmpeg is already configured.
///
/// Returns `Ok` with `ColorMetadata::fallback()` even when parsing
/// fails — this is best-effort and a wrong-but-plausible default beats
/// failing the encode entirely.
pub fn probe_color_metadata(ffmpeg_path: &str, file: &Path) -> ColorMetadata {
    let out = ffmpeg_command(ffmpeg_path)
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-i")
        .arg(file)
        .arg("-t")
        .arg("0")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match out {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            parse_color_metadata(&stderr).unwrap_or_else(ColorMetadata::fallback)
        }
        Err(_) => ColorMetadata::fallback(),
    }
}

/// Parse a `Stream #...: Video: <codec> ..., <pix_fmt>(<color>...), WxH`
/// line out of ffmpeg's stderr. The color block inside parens varies:
/// `(pc)`, `(pc, bt709)`, or `(pc, bt470bg/bt470bg/smpte170m)` — range,
/// then optionally a slash-tuple of primaries/space/trc (or a single
/// shared tag). Returns `None` if no Video stream line is found or the
/// pix_fmt token can't be located.
fn parse_color_metadata(stderr: &str) -> Option<ColorMetadata> {
    let line = stderr.lines().find(|l| l.contains("Video:"))?;
    let after = line.split_once("Video:")?.1.trim();

    // Split on commas at paren-depth zero so the codec descriptor's
    // own `(profile)` and `(tag / 0xhhh)` pieces don't get chopped.
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    for ch in after.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = (depth - 1).max(0);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    tokens.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let last = current.trim();
    if !last.is_empty() {
        tokens.push(last.to_string());
    }

    // First pix_fmt-shaped token wins. The codec descriptor sits before
    // it; the `WxH` resolution token sits after.
    let pix_prefixes = [
        "yuv", "yuyv", "yvyu", "uyvy", "uyy", "nv", "rgb", "bgr", "gbr",
        "gray", "monoblack", "monowhite", "monob", "monow", "pal8", "ya",
    ];
    let mut pix_fmt: Option<String> = None;
    let mut color_block: Option<String> = None;
    for t in &tokens {
        let head = t.split('(').next().unwrap_or("").trim();
        if pix_prefixes.iter().any(|p| head.starts_with(p)) {
            pix_fmt = Some(head.to_string());
            if let (Some(open), Some(close)) = (t.find('('), t.rfind(')')) {
                if close > open + 1 {
                    color_block = Some(t[open + 1..close].to_string());
                }
            }
            break;
        }
    }

    let pix_fmt = pix_fmt?;

    let mut color_range: Option<String> = None;
    let mut color_primaries: Option<String> = None;
    let mut color_trc: Option<String> = None;
    let mut color_space: Option<String> = None;

    if let Some(block) = color_block {
        let parts: Vec<&str> = block.split(',').map(|s| s.trim()).collect();
        if let Some(r) = parts.first() {
            if *r == "pc" || *r == "tv" {
                color_range = Some((*r).to_string());
            }
        }
        if parts.len() > 1 {
            let colors = parts[1];
            let cparts: Vec<&str> = colors.split('/').map(|s| s.trim()).collect();
            match cparts.len() {
                1 => {
                    color_primaries = Some(cparts[0].to_string());
                    color_trc = Some(cparts[0].to_string());
                    color_space = Some(cparts[0].to_string());
                }
                3 => {
                    // ffmpeg's avcodec_string prints the 3-tuple as
                    // primaries/colorspace/trc — not primaries/trc/space
                    // as the slash order intuitively suggests. Verified
                    // empirically against the Wolf Box interior cam:
                    // ffprobe says primaries=bt470bg, space=bt470bg,
                    // trc=smpte170m and the stream line reads
                    // `bt470bg/bt470bg/smpte170m`.
                    color_primaries = Some(cparts[0].to_string());
                    color_space = Some(cparts[1].to_string());
                    color_trc = Some(cparts[2].to_string());
                }
                _ => {}
            }
        }
    }

    Some(ColorMetadata {
        pix_fmt,
        color_range,
        color_primaries,
        color_trc,
        color_space,
    })
}

/// Produce a short black-frame MP4 at the target `output` path with
/// matching codec/resolution/framerate so it can be fed alongside the
/// real channel files through the ffmpeg concat demuxer.
///
/// The concat demuxer requires all inputs to share codec + pixel
/// format + resolution + framerate, AND the decoded frame parameters
/// (range/colorspace) need to match the rest of the concat to keep
/// ffmpeg's auto-scaler from tripping on a reinit it can't perform.
/// Callers probe a reference real sibling via `mp4_probe` (for
/// resolution + fps) and `probe_color_metadata` (for pix_fmt + color
/// tags) and hand those params in here so the placeholder sits flush
/// with its neighbors in the list.
#[allow(clippy::too_many_arguments)]
pub fn generate_black_placeholder(
    ffmpeg_path: &str,
    output: &Path,
    width: u32,
    height: u32,
    fps: u32,
    duration_s: f64,
    encoder: Encoder,
    color: &ColorMetadata,
) -> Result<(), AppError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    if output.exists() {
        let _ = fs::remove_file(output);
    }

    // `lavfi color=black` defaults to yuv420p; chain a `format=` filter
    // so the source stream comes out in the reference sibling's pix_fmt
    // (typically yuvj420p for Wolf Box).
    let color_arg = format!(
        "color=black:size={}x{}:rate={}:duration={:.3},format={}",
        width, height, fps, duration_s, color.pix_fmt,
    );

    let mut cmd = ffmpeg_command(ffmpeg_path);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error");

    // Color tags set BEFORE `-i` tag the input stream's frames at the
    // source. Critical: the same flags placed only after `-c:v` get
    // half-honored on the NVENC path — `color_range` and `colorspace`
    // make it through but `color_primaries` and `color_trc` are dropped,
    // leaving the placeholder partially tagged and still mismatched
    // against the real Wolf Box footage. Setting them at the input side
    // means the encoder sees fully-tagged frames and writes them all
    // through to the bitstream / container.
    if let Some(r) = &color.color_range {
        cmd.arg("-color_range").arg(r);
    }
    if let Some(p) = &color.color_primaries {
        cmd.arg("-color_primaries").arg(p);
    }
    if let Some(t) = &color.color_trc {
        cmd.arg("-color_trc").arg(t);
    }
    if let Some(s) = &color.color_space {
        cmd.arg("-colorspace").arg(s);
    }

    cmd.arg("-f")
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
    fn parse_color_metadata_extracts_pix_fmt_and_full_color_tuple() {
        // Wolf Box front cam (4K, full range, BT.709 across all three).
        let stderr = "  Stream #0:0[0x1](und): Video: hevc (Main 10) (hev1 / 0x31766568), \
            yuvj420p(pc, bt709), 3840x2160 [SAR 1:1 DAR 16:9], 25500 kb/s, 30 fps, \
            30 tbr, 90k tbn (default)\n";
        let m = parse_color_metadata(stderr).expect("should parse");
        assert_eq!(m.pix_fmt, "yuvj420p");
        assert_eq!(m.color_range.as_deref(), Some("pc"));
        // Single-tag color block expands to all three slots so the
        // muxer flags get applied uniformly.
        assert_eq!(m.color_primaries.as_deref(), Some("bt709"));
        assert_eq!(m.color_trc.as_deref(), Some("bt709"));
        assert_eq!(m.color_space.as_deref(), Some("bt709"));
    }

    #[test]
    fn parse_color_metadata_extracts_split_color_tuple() {
        // Wolf Box interior cam, observed verbatim: primaries=bt470bg,
        // space=bt470bg, trc=smpte170m. Print order is primaries/space/
        // trc per ffmpeg's avcodec_string formatting.
        let stderr = "Stream #0:0[0x1](eng): Video: hevc (Main) (hvc1 / 0x31637668), \
            yuvj420p(pc, bt470bg/bt470bg/smpte170m), 1920x1080, 5884 kb/s, \
            25 fps, 25 tbr, 120k tbn (default)\n";
        let m = parse_color_metadata(stderr).expect("should parse");
        assert_eq!(m.pix_fmt, "yuvj420p");
        assert_eq!(m.color_range.as_deref(), Some("pc"));
        assert_eq!(m.color_primaries.as_deref(), Some("bt470bg"));
        assert_eq!(m.color_space.as_deref(), Some("bt470bg"));
        assert_eq!(m.color_trc.as_deref(), Some("smpte170m"));
    }

    #[test]
    fn parse_color_metadata_handles_range_only_block() {
        let stderr =
            "Stream #0:0: Video: h264, yuv420p(tv), 1280x720, 30 fps\n";
        let m = parse_color_metadata(stderr).expect("should parse");
        assert_eq!(m.pix_fmt, "yuv420p");
        assert_eq!(m.color_range.as_deref(), Some("tv"));
        assert!(m.color_primaries.is_none());
        assert!(m.color_trc.is_none());
        assert!(m.color_space.is_none());
    }

    #[test]
    fn parse_color_metadata_handles_no_color_block() {
        // Older / untagged files — pix_fmt with no parens after.
        let stderr =
            "Stream #0:0: Video: h264, yuv420p, 1280x720, 30 fps\n";
        let m = parse_color_metadata(stderr).expect("should parse");
        assert_eq!(m.pix_fmt, "yuv420p");
        assert!(m.color_range.is_none());
    }

    #[test]
    fn parse_color_metadata_returns_none_without_video_stream() {
        let stderr = "Input #0, mp3, from 'foo.mp3':\n  \
            Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 192 kb/s\n";
        assert!(parse_color_metadata(stderr).is_none());
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
            &ColorMetadata::fallback(),
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
