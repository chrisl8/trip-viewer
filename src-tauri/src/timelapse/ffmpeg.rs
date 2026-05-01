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

    // Remove any stale output file from a previous failed attempt so
    // ffmpeg doesn't inherit a partial moov atom or confuse muxers.
    if args.output_path.exists() {
        let _ = fs::remove_file(args.output_path);
    }

    if args.curve.len() <= 1 {
        encode_single_shot(args, cancel)
    } else {
        encode_multi_window(args, cancel)
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
    let filter = speed_curve::compose_window_filter(args.encoder.scale_filter(), rate);

    let mut cmd = ffmpeg_command(args.ffmpeg_path);
    apply_loglevel_flags(&mut cmd);

    if args.encoder.needs_cuda_hwaccel() {
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

    // Phase 2: per-window encode. Each ffmpeg consumes the prepared
    // source with a fast input seek, scales, applies the segment's
    // rate, and writes a small MP4. Sequential within the job so we
    // don't undo the memory savings by running them in parallel.
    let mut window_paths: Vec<PathBuf> = Vec::with_capacity(args.curve.len());
    let mut window_err: Option<AppError> = None;
    for (i, seg) in args.curve.iter().enumerate() {
        let duration = (seg.concat_end - seg.concat_start).max(0.0);
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
    output: &Path,
    ffmpeg_path: &str,
    cpu_pool_threads: Option<usize>,
    cancel: &CancelFlag,
) -> Result<(), AppError> {
    if output.exists() {
        let _ = fs::remove_file(output);
    }

    let filter = speed_curve::compose_window_filter(encoder.scale_filter(), rate);

    let mut cmd = ffmpeg_command(ffmpeg_path);
    apply_loglevel_flags(&mut cmd);

    if encoder.needs_cuda_hwaccel() {
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
fn drain_to_tail<R: std::io::Read>(mut reader: R, max_bytes: usize) -> Vec<u8> {
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
