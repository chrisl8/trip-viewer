//! Thin wrapper around `std::process::Command` for the ffmpeg binary.
//!
//! Two entry points:
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
//!
//! Missing sibling channels are no longer plugged with black-frame
//! placeholders — a channel that's off for part of a trip is encoded
//! from its real footage only, and the player holds + black-overlays it
//! across the gaps (see `speed_curve::restrict_curve_to_coverage`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::error::AppError;
use crate::timelapse::speed_curve::{self, CurveSegment};
use crate::timelapse::types::{Channel, FfmpegCapabilities, Tier};
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

/// macOS only: returns true if the file at `path` has the
/// `com.apple.quarantine` extended attribute. Files downloaded from
/// the internet (Safari, browser, AirDrop) get this attribute set;
/// Gatekeeper then refuses to run unsigned/un-notarized binaries that
/// carry it. `xattr -p` exits 0 when the attribute is present, non-zero
/// when it's not. Any other failure (file missing, xattr binary missing
/// on a stripped system) is treated as "not quarantined" so the caller
/// surfaces the underlying probe error rather than a misleading
/// quarantine prompt.
#[cfg(target_os = "macos")]
pub fn has_quarantine_attr(path: &str) -> bool {
    Command::new("xattr")
        .arg("-p")
        .arg("com.apple.quarantine")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// macOS only: strips `com.apple.quarantine` from `path`. Treats
/// "already absent" as success so retries are idempotent.
#[cfg(target_os = "macos")]
pub fn clear_quarantine_attr(path: &str) -> Result<(), AppError> {
    let output = Command::new("xattr")
        .arg("-d")
        .arg("com.apple.quarantine")
        .arg(path)
        .output()
        .map_err(|e| AppError::Internal(format!("could not run xattr: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such xattr") {
        return Ok(());
    }
    Err(AppError::Internal(format!(
        "xattr -d failed: {}",
        stderr.trim()
    )))
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
    #[allow(dead_code)] // surfaced via EncodeArgs for future logging/metrics
    pub tier: Tier,
    #[allow(dead_code)] // referenced by future log lines and metrics
    pub channel: Channel,
    pub encoder: Encoder,
    /// Pre-built speed curve. The dispatcher in `encode_trip_channel`
    /// reads `curve.len()` to choose between the single-shot filter
    /// graph (1 segment) and the multi-window pipeline (2+ segments).
    /// Worker builds the curve once per job and reuses it for the
    /// persisted JSON metadata, so we accept it as input rather than
    /// rebuilding from `(windows, tier, total_duration)`.
    pub curve: &'a [CurveSegment],
    /// Per-job scratch directory for temp files. The multi-window
    /// path writes a stream-copied source MP4 and one MP4 per curve
    /// segment here, then deletes them on success or failure. Caller
    /// guarantees the directory exists and sweeps any leftover files
    /// after `encode_trip_channel` returns.
    pub scratch_dir: &'a Path,
    /// Cap on the per-encode CPU thread pool. Honored only by the
    /// `LibX265` path (NVENC's encode threads are GPU-side). Used when
    /// the worker runs N parallel ffmpegs to keep the combined x265
    /// thread count near the host's logical-core count instead of N×
    /// oversubscribing it. `None` = let x265 pick its own pool size.
    pub cpu_pool_threads: Option<usize>,
    /// Force software decode + `scale` (CPU) filter even when `encoder`
    /// is `HevcNvenc`. NVENC encoding stays GPU-side. Caller sets this
    /// for jobs whose concat source mixes black-placeholder segments
    /// with real Wolf Box footage: NVDEC + the auto-inserted `scale_cuda`
    /// scaler can't reinit when stream parameters shift at the
    /// placeholder→real boundary (SAR, tbn, even sub-bitstream HEVC SPS
    /// differences trip it), surfacing as `auto_scaler reinit -38
    /// ENOSYS`. Software decode tolerates the boundary cleanly. Per-job
    /// flag rather than a global setting so the common (no-pad) case
    /// keeps the fast end-to-end CUDA path.
    pub software_input: bool,
}

/// Encode one (trip, tier, channel) output. Blocks until the encode
/// completes, polling `cancel` every 500ms; if cancelled, kills any
/// in-flight ffmpeg child and deletes partial output before returning.
/// Returns `Ok(output_path)` on success.
///
/// Dispatches between two pipelines based on curve length:
/// - **Single segment** (fixed tiers, or variable tiers with no event
///   windows): one ffmpeg invocation feeding all source segments
///   through a `concat → scale → setpts` graph. Memory bounded by the
///   decoder's own queues (~2 GB).
/// - **Multi segment** (variable tiers with event windows): split into
///   three phases — stream-copy the sources into a temp single MP4,
///   encode each curve segment from that source as its own small
///   ffmpeg, then stream-copy the per-window outputs into the final
///   file. The per-process memory profile is bounded by a single-
///   stream pipeline regardless of how many windows the curve has —
///   the previous `split=N → trim → concat=N` graph buffered frames
///   on inactive concat inputs and pinned 12–36 GB per job.
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
    fs::create_dir_all(args.scratch_dir)
        .map_err(|e| AppError::Internal(format!("scratch dir create failed: {e}")))?;

    // Encode to a temp file inside the per-job scratch dir, then
    // atomically rename onto `output_path` on success. This way a
    // failed re-encode (corrupt source, ffmpeg crash, missing sibling,
    // cancel) never destroys the previous good output — the user
    // keeps the working timelapse until a replacement is ready. Both
    // paths live under `<archive>/Timelapses/` (scratch is the `.tmp/`
    // subtree), so the rename is in-filesystem and atomic.
    let tmp_output = args.scratch_dir.join("__output.mp4");
    if tmp_output.exists() {
        let _ = fs::remove_file(&tmp_output);
    }

    let tmp_args = EncodeArgs {
        ffmpeg_path: args.ffmpeg_path,
        source_paths: args.source_paths,
        output_path: &tmp_output,
        tier: args.tier,
        channel: args.channel,
        encoder: args.encoder,
        curve: args.curve,
        scratch_dir: args.scratch_dir,
        cpu_pool_threads: args.cpu_pool_threads,
        software_input: args.software_input,
    };

    let encode_result = if args.curve.len() <= 1 {
        encode_single_shot(&tmp_args, cancel)
    } else {
        encode_multi_window(&tmp_args, cancel)
    };

    match encode_result {
        Ok(_) => match fs::rename(&tmp_output, args.output_path) {
            Ok(()) => Ok(args.output_path.to_path_buf()),
            Err(e) => {
                let _ = fs::remove_file(&tmp_output);
                Err(AppError::Internal(format!(
                    "could not move encoded output into place: {e}"
                )))
            }
        },
        Err(e) => {
            let _ = fs::remove_file(&tmp_output);
            Err(e)
        }
    }
}

/// Two-phase pipeline for single-segment curves: stream-copy the
/// input segments into a single MP4 via the concat demuxer, then
/// encode that single source with a one-input filter graph. This
/// replaces the older "N `-i` inputs through a `concat` filter"
/// approach, which allocated a separate NVDEC context per input —
/// observed ~200 MB of host RAM per `-hwaccel cuda` input, scaling
/// linearly with segment count and pushing a 200-segment trip to
/// 40 GB resident.
///
/// With one prepared source there's exactly one decoder context
/// regardless of how many segments the trip has. Stream-copy concat
/// also means phase 1 doesn't decode at all, so memory there is
/// trivial.
fn encode_single_shot(args: &EncodeArgs<'_>, cancel: &CancelFlag) -> Result<PathBuf, AppError> {
    fs::create_dir_all(args.scratch_dir)
        .map_err(|e| AppError::Internal(format!("scratch dir create failed: {e}")))?;

    // Phase 1: source prep. Concat-demuxer + `-c copy` produces a
    // single MP4 holding every input segment back-to-back, with no
    // re-encode. Wolf Box recordings + matched-parameter black
    // placeholders share codec/resolution/fps/pix_fmt by construction,
    // so the demuxer accepts them without normalization.
    let source_path = args.scratch_dir.join("__single_source.mp4");
    if let Err(e) = prepare_concat_source(args.source_paths, &source_path, args.ffmpeg_path, cancel)
    {
        if source_path.exists() {
            let _ = fs::remove_file(&source_path);
        }
        return Err(e);
    }

    // Phase 2: one ffmpeg, one input, one decoder context. Filter
    // is the same simple `scale + setpts` shape used for per-window
    // encodes in the multi-window path; the single-segment curve's
    // rate determines the speed factor.
    let rate = args.curve.first().map(|s| s.rate).unwrap_or(1);
    let scale_filter = if args.software_input { "scale" } else { args.encoder.scale_filter() };
    let filter = speed_curve::compose_window_filter(scale_filter, rate);

    let mut cmd = ffmpeg_command(args.ffmpeg_path);
    apply_loglevel_flags(&mut cmd);

    if args.encoder.needs_cuda_hwaccel() && !args.software_input {
        cmd.arg("-hwaccel")
            .arg("cuda")
            .arg("-hwaccel_output_format")
            .arg("cuda");
    }

    cmd.arg("-i")
        .arg(&source_path)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[out]")
        .arg("-an");

    apply_encoder_flags(&mut cmd, args.encoder, args.cpu_pool_threads);

    cmd.arg(args.output_path);

    let result = run_ffmpeg_with_cancel(cmd, cancel);
    let _ = fs::remove_file(&source_path);

    match result {
        Ok(()) => Ok(args.output_path.to_path_buf()),
        Err(e) => {
            if args.output_path.exists() {
                let _ = fs::remove_file(args.output_path);
            }
            Err(e)
        }
    }
}

/// Three-phase pipeline that fans the per-window encodes out into
/// independent ffmpeg invocations instead of a single big filter
/// graph. Trades a few extra spawns for a per-process memory profile
/// that's bounded by a single decoder pipeline (~1–2 GB) instead of
/// the `split=N` fan-out's `~N × frame_buffer` cost.
fn encode_multi_window(args: &EncodeArgs<'_>, cancel: &CancelFlag) -> Result<PathBuf, AppError> {
    fs::create_dir_all(args.scratch_dir).map_err(|e| {
        AppError::Internal(format!("scratch dir create failed: {e}"))
    })?;

    let source_path = args.scratch_dir.join("__multi_source.mp4");
    let prep_result = prepare_concat_source(
        args.source_paths,
        &source_path,
        args.ffmpeg_path,
        cancel,
    );
    if let Err(e) = prep_result {
        if source_path.exists() {
            let _ = fs::remove_file(&source_path);
        }
        return Err(e);
    }

    // Probe the prepared source's real duration. The curve is derived
    // from front-segment durations; if a channel's real footage is even
    // slightly shorter (clock skew, or a stub sibling that slipped past
    // the coverage check), a tail window could seek at/after EOF, decode
    // zero frames, and the CUDA filter never inits → NVENC fails to open
    // ("hw_frames_ctx must be set" / -22). Clamp every window's seek+len
    // to the real source so that can never happen. Probe failure → no
    // clamp (treat as unbounded; preserves prior behavior).
    let source_dur = crate::metadata::mp4_probe::probe(&source_path)
        .map(|m| m.duration_s)
        .unwrap_or(f64::INFINITY);

    // Phase 2: per-window encode. Each ffmpeg consumes the prepared
    // source with a fast input seek, scales, applies the segment's
    // rate, and writes a small MP4. Sequential within the job so we
    // don't undo the memory savings by running them in parallel.
    let mut window_paths: Vec<PathBuf> = Vec::with_capacity(args.curve.len());
    let mut window_err: Option<AppError> = None;
    for (i, seg) in args.curve.iter().enumerate() {
        let mut duration = (seg.concat_end - seg.concat_start).max(0.0);
        // Skip a window that starts at/after the real source end, and
        // truncate one that straddles it — either way no zero-frame seek.
        if seg.concat_start >= source_dur {
            eprintln!(
                "[timelapse] skipping window {i} at {:.3}s — past source end {:.3}s",
                seg.concat_start, source_dur
            );
            continue;
        }
        if seg.concat_start + duration > source_dur {
            duration = source_dur - seg.concat_start;
        }
        if duration <= 0.0 {
            // build_curve drops zero-width segments today, but if a
            // future change leaks one through, skipping it here keeps
            // the pipeline robust rather than failing the whole job.
            continue;
        }
        let window_path = args.scratch_dir.join(format!("__multi_window_{i}.mp4"));
        match encode_window(
            &source_path,
            seg.concat_start,
            duration,
            seg.rate,
            args.encoder,
            args.software_input,
            &window_path,
            args.ffmpeg_path,
            args.cpu_pool_threads,
            cancel,
        ) {
            Ok(()) => window_paths.push(window_path),
            Err(e) => {
                window_err = Some(e);
                break;
            }
        }
    }

    // Source MP4 is no longer needed once the per-window encodes have
    // finished (or aborted). Free the disk regardless of outcome.
    let _ = fs::remove_file(&source_path);

    if let Some(e) = window_err {
        for w in &window_paths {
            let _ = fs::remove_file(w);
        }
        return Err(e);
    }

    if window_paths.is_empty() {
        return Err(AppError::Internal(
            "multi-window encode produced no segments — curve had no usable spans".into(),
        ));
    }

    // Phase 3: stream-copy concat the per-window outputs into the
    // final file. No re-encode, no decode — purely muxing.
    let result = concat_window_outputs(
        &window_paths,
        args.output_path,
        args.ffmpeg_path,
        cancel,
    );
    for w in &window_paths {
        let _ = fs::remove_file(w);
    }
    match result {
        Ok(()) => Ok(args.output_path.to_path_buf()),
        Err(e) => {
            if args.output_path.exists() {
                let _ = fs::remove_file(args.output_path);
            }
            Err(e)
        }
    }
}

/// Phase 1 of the multi-window pipeline: stream-copy the input
/// segments into a single MP4 via the concat demuxer. No re-encode
/// — ffmpeg runs in pure mux mode so memory and CPU are trivial.
/// Output is the same total bitrate as the inputs combined.
fn prepare_concat_source(
    sources: &[String],
    output: &Path,
    ffmpeg_path: &str,
    cancel: &CancelFlag,
) -> Result<(), AppError> {
    let parent = output.parent().ok_or_else(|| {
        AppError::Internal("multi-window source output has no parent dir".into())
    })?;
    let list_path = parent.join("__multi_source_list.txt");
    write_concat_list(sources, &list_path)?;

    let mut cmd = ffmpeg_command(ffmpeg_path);
    apply_loglevel_flags(&mut cmd);
    cmd.arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        .arg("-an")
        .arg(output);

    let result = run_ffmpeg_with_cancel(cmd, cancel);
    let _ = fs::remove_file(&list_path);
    result
}

/// Phase 2 of the multi-window pipeline: encode one curve segment as
/// its own MP4. Single input (the prepared source), single-stream
/// filter graph (`scale + setpts`), one ffmpeg per call. Uses keyframe-
/// aligned input seek (`-ss` before `-i`) for speed; HEVC GOP boundary
/// alignment of ~1 s at the segment start is acceptable because the
/// player uses the persisted curve metadata for time mapping, not
/// frame-accurate window edges.
#[allow(clippy::too_many_arguments)]
fn encode_window(
    source: &Path,
    window_start_s: f64,
    window_duration_s: f64,
    rate: u32,
    encoder: Encoder,
    software_input: bool,
    output: &Path,
    ffmpeg_path: &str,
    cpu_pool_threads: Option<usize>,
    cancel: &CancelFlag,
) -> Result<(), AppError> {
    if output.exists() {
        let _ = fs::remove_file(output);
    }

    let scale_filter = if software_input { "scale" } else { encoder.scale_filter() };
    let filter = speed_curve::compose_window_filter(scale_filter, rate);

    let mut cmd = ffmpeg_command(ffmpeg_path);
    apply_loglevel_flags(&mut cmd);

    if encoder.needs_cuda_hwaccel() && !software_input {
        cmd.arg("-hwaccel")
            .arg("cuda")
            .arg("-hwaccel_output_format")
            .arg("cuda");
    }

    cmd.arg("-ss")
        .arg(format!("{window_start_s:.3}"))
        .arg("-t")
        .arg(format!("{window_duration_s:.3}"))
        .arg("-i")
        .arg(source)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[out]")
        .arg("-an");

    apply_encoder_flags(&mut cmd, encoder, cpu_pool_threads);

    cmd.arg(output);

    match run_ffmpeg_with_cancel(cmd, cancel) {
        Ok(()) => Ok(()),
        Err(e) => {
            if output.exists() {
                let _ = fs::remove_file(output);
            }
            Err(e)
        }
    }
}

/// Phase 3 of the multi-window pipeline: stream-copy concat the
/// per-window outputs into the final MP4. No re-encode — purely a
/// muxer pass. The concat-demuxer issue noted in the module-level
/// docs (it can't survive parameter changes mid-stream when feeding
/// NVDEC) doesn't apply here: stream copy bypasses the decoder
/// entirely, and all per-window outputs were produced by the same
/// encoder with identical parameters one moment apart.
fn concat_window_outputs(
    windows: &[PathBuf],
    output: &Path,
    ffmpeg_path: &str,
    cancel: &CancelFlag,
) -> Result<(), AppError> {
    let parent = output.parent().ok_or_else(|| {
        AppError::Internal("multi-window concat output has no parent dir".into())
    })?;
    let list_path = parent.join("__multi_windows_list.txt");
    let strs: Vec<String> = windows
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    write_concat_list(&strs, &list_path)?;

    let mut cmd = ffmpeg_command(ffmpeg_path);
    apply_loglevel_flags(&mut cmd);
    cmd.arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-c")
        .arg("copy")
        .arg("-an")
        .arg(output);

    let result = run_ffmpeg_with_cancel(cmd, cancel);
    let _ = fs::remove_file(&list_path);
    result
}

/// Write a concat-demuxer list file. Paths are wrapped in single
/// quotes; embedded apostrophes are escaped per the demuxer's rules
/// (close quote, backslash-quote, reopen). Backslashes are left as
/// path separators on Windows — the demuxer treats them literally,
/// not as escape characters.
fn write_concat_list(paths: &[String], list_path: &Path) -> Result<(), AppError> {
    let mut content = String::new();
    for p in paths {
        content.push_str("file '");
        content.push_str(&p.replace('\'', "'\\''"));
        content.push_str("'\n");
    }
    fs::write(list_path, content)
        .map_err(|e| AppError::Internal(format!("failed to write concat list: {e}")))
}

/// Apply the universal ffmpeg quiet-mode flags to a Command. Used by
/// every spawn site so we don't emit progress noise that the worker's
/// stderr drain would just have to throw away.
fn apply_loglevel_flags(cmd: &mut Command) {
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error");
}

/// Apply the encoder-specific output args (codec, preset, quality).
/// Shared across the single-shot and per-window paths so the two
/// produce byte-identical encodes when fed equivalent input.
///
/// `-tag:v hvc1` is force-set on every HEVC output regardless of
/// encoder. ffmpeg's mp4 muxer defaults to writing `hev1` for libx265
/// output, but Safari / WKWebView (macOS) and parts of WebView2's
/// media path reject `hev1`-tagged HEVC and play only `hvc1`. The two
/// tags describe identical bitstreams (the parameter sets just live
/// inline vs. in the sample description); forcing `hvc1` costs
/// nothing and unlocks playback on the OS-level decoders the app
/// relies on.
fn apply_encoder_flags(cmd: &mut Command, encoder: Encoder, cpu_pool_threads: Option<usize>) {
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
            // x265's `pools=N` sizes the encoder's internal worker
            // pool. Without it, every ffmpeg spawns x265 with all-cores;
            // when the supervisor runs N parallel encodes the combined
            // thread count is N× the host's core count and the OS
            // scheduler thrashes. `pools=N` keeps each encode's slice
            // proportional. `cpu_pool_threads = None` (single-job
            // runs) lets x265 pick its own pool size unchanged.
            let mut x265_params = String::from("log-level=error");
            if let Some(threads) = cpu_pool_threads {
                if threads >= 1 {
                    x265_params.push_str(&format!(":pools={threads}"));
                }
            }
            cmd.arg("-c:v")
                .arg("libx265")
                .arg("-crf")
                .arg("26")
                .arg("-preset")
                .arg("medium")
                .arg("-x265-params")
                .arg(&x265_params);
        }
    }
    cmd.arg("-tag:v").arg("hvc1");
}

/// Spawn `cmd`, drain its stderr to a bounded tail, and poll `cancel`
/// every 500 ms until the child exits or the user aborts. On clean
/// exit returns `Ok(())`. On cancel returns
/// `Err(AppError::Internal("cancelled"))` — the literal string is
/// matched by the worker to distinguish cancel from a real failure.
/// On non-zero exit returns the last 8 lines of stderr.
///
/// The child's stdout is wired to /dev/null and stderr is piped into
/// a `drain_to_tail` reader thread; this caps captured stderr at
/// 64 KB regardless of how chatty the child gets, which is the fix
/// for the parent-process OOM that hit when CUDA exhaustion made
/// ffmpeg emit one error line per frame.
fn run_ffmpeg_with_cancel(mut cmd: Command, cancel: &CancelFlag) -> Result<(), AppError> {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Internal(format!("failed to spawn ffmpeg: {e}")))?;

    const STDERR_TAIL_BYTES: usize = 64 * 1024;
    let stderr_handle = child
        .stderr
        .take()
        .map(|s| thread::spawn(move || drain_to_tail(s, STDERR_TAIL_BYTES)));

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

    let exit_status = child
        .wait()
        .map_err(|e| AppError::Internal(format!("ffmpeg wait failed: {e}")))?;

    let stderr_tail = stderr_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();

    if cancelled {
        return Err(AppError::Internal("cancelled".into()));
    }

    if !exit_status.success() {
        let stderr = String::from_utf8_lossy(&stderr_tail);
        let tail = tail_lines(&stderr, 8);
        return Err(AppError::Internal(format!(
            "ffmpeg exited with {exit_status}: {tail}"
        )));
    }

    Ok(())
}

/// Read a child's stderr to EOF while keeping only the last
/// `max_bytes` of output in memory. Reads in 4 KB chunks; whenever the
/// buffer grows past 2× the cap, it drains the front half. The
/// amortized cost is O(total bytes read) and the steady-state memory
/// footprint is `2 * max_bytes` regardless of how chatty the child is.
pub(crate) fn drain_to_tail<R: std::io::Read>(mut reader: R, max_bytes: usize) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(max_bytes.saturating_mul(2));
    let high_water = max_bytes.saturating_mul(2).max(8192);
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > high_water {
                    let drop = buf.len() - max_bytes;
                    buf.drain(..drop);
                }
            }
        }
    }
    if buf.len() > max_bytes {
        let drop = buf.len() - max_bytes;
        buf.drain(..drop);
    }
    buf
}

pub(crate) fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

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

}
