//! Runs one ffmpeg export, reporting progress and honouring cancel.
//!
//! Same idea as `timelapse::ffmpeg::run_ffmpeg_with_cancel`, plus a reader
//! on ffmpeg's `-progress pipe:1` output so the UI can show a percentage.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::error::AppError;
use crate::timelapse::ffmpeg::{drain_to_tail, tail_lines};
use crate::timelapse::CancelFlag;

/// Parse one line of `-progress` output into elapsed output seconds.
///
/// ffmpeg prints `out_time_us=<microseconds>` and, for historical
/// reasons, `out_time_ms` which is *also* microseconds. Both are accepted.
/// `N/A` (printed before the first frame) yields `None`.
pub fn parse_progress_line(line: &str) -> Option<f64> {
    let value = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))?;
    let micros: f64 = value.trim().parse().ok()?;
    if micros < 0.0 {
        return None;
    }
    Some(micros / 1_000_000.0)
}

/// Spawn `cmd` (already carrying `-progress pipe:1`), call `on_progress`
/// with a 0..=1 fraction as ffmpeg advances, and poll `cancel` every
/// 200 ms. On cancel the child is killed and `Err("cancelled")` is
/// returned. On failure the last lines of stderr are included.
pub fn run_with_progress(
    mut cmd: Command,
    total_s: f64,
    cancel: &CancelFlag,
    on_progress: impl Fn(f64) + Send + 'static,
) -> Result<(), AppError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Internal(format!("failed to spawn ffmpeg: {e}")))?;

    const STDERR_TAIL_BYTES: usize = 64 * 1024;
    let stderr_handle = child
        .stderr
        .take()
        .map(|s| thread::spawn(move || drain_to_tail(s, STDERR_TAIL_BYTES)));
    let stdout_handle = child.stdout.take().map(|o| {
        thread::spawn(move || {
            for line in BufReader::new(o).lines().map_while(Result::ok) {
                if let Some(secs) = parse_progress_line(&line) {
                    if total_s > 0.0 {
                        on_progress((secs / total_s).clamp(0.0, 1.0));
                    }
                }
            }
        })
    });

    let cancelled = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                return Err(AppError::Internal(format!("error waiting on ffmpeg: {e}")));
            }
        }
    };

    let status = child
        .wait()
        .map_err(|e| AppError::Internal(format!("ffmpeg wait failed: {e}")))?;
    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    let stderr_tail = stderr_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();

    if cancelled {
        return Err(AppError::Internal("cancelled".into()));
    }
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_tail);
        return Err(AppError::Internal(format!(
            "ffmpeg exited with {status}: {}",
            tail_lines(&stderr, 8)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[test]
    fn parses_microsecond_progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=1500000"), Some(1.5));
        assert_eq!(parse_progress_line("out_time_ms=2000000"), Some(2.0));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("out_time_us=-5"), None);
        assert_eq!(parse_progress_line("frame=12"), None);
        assert_eq!(parse_progress_line("progress=end"), None);
    }

    #[cfg(unix)]
    #[test]
    fn reports_progress_and_succeeds() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo out_time_us=500000; echo out_time_us=1000000; echo progress=end");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let cancel: CancelFlag = Arc::new(AtomicBool::new(false));
        run_with_progress(cmd, 1.0, &cancel, move |f| seen2.lock().unwrap().push(f)).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![0.5, 1.0]);
    }

    #[cfg(unix)]
    #[test]
    fn failure_carries_the_stderr_tail() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo boom >&2; exit 3");
        let cancel: CancelFlag = Arc::new(AtomicBool::new(false));
        let e = run_with_progress(cmd, 1.0, &cancel, |_| {}).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("boom"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn cancel_kills_the_child_and_reports_cancelled() {
        let mut cmd = Command::new("sh");
        // `exec` so the shell is replaced by sleep: killing the child then
        // closes its pipes (ffmpeg is spawned directly, with no shell).
        cmd.arg("-c").arg("exec sleep 30");
        let cancel: CancelFlag = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            c2.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let e = run_with_progress(cmd, 1.0, &cancel, |_| {}).unwrap_err();
        t.join().unwrap();
        assert!(e.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
