//! Pick a worker-pool size for the timelapse pipeline based on the
//! encoder and the host hardware.
//!
//! Sequential encoding leaves most of the GPU (or CPU) on the floor when
//! there's a backlog of jobs to chew through. This module returns a
//! conservative-but-useful N for the worker loop in `worker.rs` to
//! dispatch with — bounded by three independent caps and the smallest
//! one wins:
//!
//! 1. **Encoder/GPU cap.** NVENC session count + VRAM headroom; libx265
//!    saturates ~4 cores so logical_cores/4 is the natural cap.
//! 2. **System-RAM cap.** Each parallel ffmpeg's filter graph + decoder
//!    queues run ~10–12 GB of host RAM on Wolf Box-shaped 4K input.
//!    On a 16 GB box, 2 parallel jobs OOM the parent.
//! 3. **Hard ceiling of 4.** Beyond that, VRAM/disk-IO contention
//!    dominates the throughput win and failure modes (NVENC session-
//!    limit, fragmented disk reads) get harder to diagnose.

use std::process::Command;

use crate::timelapse::ffmpeg::Encoder;

/// Hard ceiling on auto-detected concurrency. A user can still override
/// via the `timelapse_max_concurrent_jobs` setting, but the auto path
/// won't exceed this regardless of how beefy the hardware looks.
pub const MAX_CONCURRENCY: usize = 4;

/// Returns a recommended worker-pool size for the given encoder. Always
/// in `1..=MAX_CONCURRENCY`. Falls back to a safe default (`2`) when
/// hardware introspection fails — better to under-utilize than to hit
/// a CUDA OOM mid-batch.
///
/// Two-stage clamp: encoder-side cap (GPU sessions / CPU cores) then
/// host-RAM cap. On a 64 GB box with an RTX 4090 the encoder cap of `2`
/// wins; on a 16 GB box with the same GPU, the RAM cap drops it to `1`.
pub fn detect_recommended_concurrency(encoder: Encoder) -> usize {
    let raw = match encoder {
        Encoder::HevcNvenc => detect_nvenc_concurrency(),
        Encoder::LibX265 => detect_libx265_concurrency(),
        // TODO when VideoToolbox/AMF/QSV land — each has its own
        // session/queue characteristics worth modeling here.
    };
    let encoder_capped = raw.clamp(1, MAX_CONCURRENCY);
    apply_ram_cap(encoder_capped)
}

/// Cap concurrency by available system RAM. Each parallel ffmpeg job
/// is budgeted at `PER_JOB_GB` of host RAM — empirical figure observed
/// on Wolf Box 4K HEVC input running through the variable-speed
/// concat/split filter graph. `OS_RESERVE_GB` keeps room for the OS,
/// the dev tooling, and the rest of the app on top of the encode pool.
///
/// If RAM probing fails we trust the encoder-side cap unchanged. The
/// per-job budget is intentionally pessimistic — under-utilizing on a
/// chunky workstation is a smaller bug than OOM-killing the parent on
/// a stock 32 GB laptop.
fn apply_ram_cap(requested: usize) -> usize {
    const PER_JOB_GB: u64 = 12;
    const OS_RESERVE_GB: u64 = 8;
    let total_gb = match total_ram_bytes() {
        Some(b) => b / (1024 * 1024 * 1024),
        None => return requested,
    };
    let available = total_gb.saturating_sub(OS_RESERVE_GB);
    let allowed = (available / PER_JOB_GB) as usize;
    requested.min(allowed.max(1))
}

fn detect_nvenc_concurrency() -> usize {
    match probe_nvidia_gpu_name() {
        Some(name) => classify_nvidia_gpu(&name),
        // No nvidia-smi → conservative default. The encoder probe in
        // ffmpeg said NVENC was available, so something is there; we
        // just can't tell what generation it is.
        None => 2,
    }
}

fn detect_libx265_concurrency() -> usize {
    // libx265 at `medium` preset saturates ~4 cores. Divide logical
    // core count by 4 so each parallel ffmpeg gets a useful slice and
    // we don't oversubscribe the OS scheduler. Pair this with a
    // matching `pools=N` value on the ffmpeg command line so x265's
    // own thread pool stays within its share.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores / 4).max(1)
}

/// Run `nvidia-smi -L` and pull the human-readable GPU model name out
/// of the first line. Returns `None` if the binary is missing, exits
/// non-zero, or produces output we can't parse.
fn probe_nvidia_gpu_name() -> Option<String> {
    let out = silent_command("nvidia-smi").arg("-L").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_first_gpu_name(&text)
}

/// Parse the first GPU name out of `nvidia-smi -L` output. The format
/// is one line per GPU:
///   `GPU 0: NVIDIA GeForce RTX 3080 Ti (UUID: GPU-...)`
/// We strip the leading `GPU N:` and trailing `(UUID: ...)`.
fn parse_first_gpu_name(text: &str) -> Option<String> {
    let line = text.lines().next()?.trim();
    let after_colon = line.split_once(": ")?.1;
    let before_paren = after_colon.split('(').next()?.trim();
    if before_paren.is_empty() {
        None
    } else {
        Some(before_paren.to_string())
    }
}

/// Map a GPU model string to a session count. Pessimistic by default:
/// **any** consumer card gets `2`, regardless of NVENC session cap.
/// Why: the binding constraint on consumer NVIDIA isn't NVENC sessions
/// (modern cards allow 3+), it's VRAM. A 12 GB RTX 3080 Ti running
/// three parallel 4K HEVC encodes hits CUDA OOM — each NVDEC + filter-
/// graph CUDA buffer + NVENC session pulls 3–4 GB of VRAM. 2-way
/// concurrency is the highest that fits on every consumer card we
/// expect to see (8 GB minimum on the modern lineup).
///
/// Pro / data-center cards (RTX A-series, Quadro, Tesla) typically
/// ship with 16–80 GB VRAM and unlocked session caps, so they get
/// MAX_CONCURRENCY. The system-RAM cap downstream of this still
/// applies — a Quadro card in a 16 GB workstation will be RAM-capped
/// to 1.
fn classify_nvidia_gpu(name: &str) -> usize {
    let lower = name.to_lowercase();

    if lower.contains("rtx a") || lower.contains("quadro") || lower.contains("tesla") {
        return MAX_CONCURRENCY;
    }

    // All consumer NVIDIA — RTX 30/40/50, RTX 20, GTX 10/16, and the
    // long tail of unrecognized names — gets 2. The simplification is
    // intentional: VRAM is the limiting reagent and we'd rather under-
    // utilize a 24 GB 4090 than OOM an 8 GB 3060.
    2
}

/// Spawn a child process without flashing a console window on Windows.
/// Mirrors `ffmpeg::ffmpeg_command` — kept local so this module stays
/// independent of the ffmpeg invocation path.
#[cfg(windows)]
fn silent_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
fn silent_command(program: &str) -> Command {
    Command::new(program)
}

/// Total physical RAM in bytes. Returns `None` on probe failure or on
/// a platform we don't have inline detection for; callers treat that
/// as "skip the RAM cap" so detection failure can't silently throttle
/// concurrency to 1 on a beefy box.
#[cfg(windows)]
fn total_ram_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    // SAFETY: zero-initialize MEMORYSTATUSEX, set dwLength as the API
    // requires, hand a pointer to the kernel. GlobalMemoryStatusEx
    // writes its result into the struct and returns nonzero on
    // success.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok != 0 {
        Some(status.ullTotalPhys)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn total_ram_bytes() -> Option<u64> {
    // /proc/meminfo's MemTotal is in kibibytes. The line shape is
    // `MemTotal:       16234556 kB` with variable whitespace.
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.trim().split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn total_ram_bytes() -> Option<u64> {
    // sysctl hw.memsize returns total physical RAM in bytes. Wired
    // up minimally here so the macOS port (when it ships) gets a
    // working RAM cap without a separate change.
    use std::ffi::CString;
    let key = CString::new("hw.memsize").ok()?;
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: passing a valid CString pointer, a writable u64, and
    // its size. sysctlbyname returns 0 on success and writes into
    // both `value` and `size`.
    let rc = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            &mut value as *mut u64 as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Some(value)
    } else {
        None
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn total_ram_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_consumer_nvidia_gets_two() {
        // VRAM (not session count) is the binding constraint, so every
        // consumer card collapses to the same conservative number.
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce RTX 3080 Ti"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce RTX 3060"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce RTX 4090"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce RTX 4070 Ti"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce GTX 1660 Ti"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce GTX 1080"), 2);
        assert_eq!(classify_nvidia_gpu("NVIDIA GeForce RTX 2070 SUPER"), 2);
    }

    #[test]
    fn classifies_pro_cards() {
        assert_eq!(classify_nvidia_gpu("NVIDIA RTX A6000"), MAX_CONCURRENCY);
        assert_eq!(classify_nvidia_gpu("Quadro RTX 8000"), MAX_CONCURRENCY);
        assert_eq!(classify_nvidia_gpu("Tesla T4"), MAX_CONCURRENCY);
    }

    #[test]
    fn unknown_gpu_falls_back_to_two() {
        assert_eq!(classify_nvidia_gpu("Some Unknown GPU"), 2);
        assert_eq!(classify_nvidia_gpu(""), 2);
    }

    #[test]
    fn parses_standard_nvidia_smi_line() {
        let sample = "GPU 0: NVIDIA GeForce RTX 3080 Ti (UUID: GPU-abc123)\n";
        assert_eq!(
            parse_first_gpu_name(sample).as_deref(),
            Some("NVIDIA GeForce RTX 3080 Ti")
        );
    }

    #[test]
    fn parses_first_of_multiple_gpus() {
        let sample = "GPU 0: NVIDIA GeForce RTX 4090 (UUID: GPU-aaa)\n\
                      GPU 1: NVIDIA GeForce RTX 4090 (UUID: GPU-bbb)\n";
        assert_eq!(
            parse_first_gpu_name(sample).as_deref(),
            Some("NVIDIA GeForce RTX 4090")
        );
    }

    #[test]
    fn rejects_garbage_output() {
        assert!(parse_first_gpu_name("").is_none());
        assert!(parse_first_gpu_name("totally not nvidia-smi output").is_none());
    }
}
