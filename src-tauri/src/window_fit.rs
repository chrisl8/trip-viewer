//! Clamp the main window to the current monitor's usable work area on startup
//! so the default size from `tauri.conf.json` never leaves the transport
//! controls hidden behind the OS taskbar on small laptop displays (issue #5).
//!
//! The v0.1.21 attempt compared `outer_size()` against the work area but then
//! wrote the result with `set_size()`, which in Tauri v2 sets the *inner*
//! (content) size. The ~31px title bar + borders were silently added back on
//! top, so the window still overflowed the work area by the frame height and
//! `center()` placed it above *and* below the taskbar. This version measures
//! the frame delta explicitly and targets an inner size whose resulting outer
//! rect fits the work area.

use tauri::{LogicalSize, WebviewWindow};

const MIN_WIDTH: f64 = 960.0;
const MIN_HEIGHT: f64 = 600.0;
#[cfg(not(windows))]
const NON_WINDOWS_HEIGHT_BUFFER: f64 = 80.0;

pub fn fit_to_work_area(window: &WebviewWindow) -> tauri::Result<()> {
    let scale = window.scale_factor()?;

    if let Some((work_w, work_h)) = work_area_logical(window, scale)? {
        let inner = window.inner_size()?;
        let outer = window.outer_size()?;
        let current_inner = (
            f64::from(inner.width) / scale,
            f64::from(inner.height) / scale,
        );
        let current_outer = (
            f64::from(outer.width) / scale,
            f64::from(outer.height) / scale,
        );

        if let Some((new_w, new_h)) = compute_target_inner(
            current_inner,
            current_outer,
            (work_w, work_h),
            (MIN_WIDTH, MIN_HEIGHT),
        ) {
            window.set_size(LogicalSize::new(new_w, new_h))?;
        }
    }

    window.center()?;
    window.show()?;
    Ok(())
}

/// Computes the target inner size (logical pixels) whose resulting outer rect
/// fits within `work`, while respecting `min_inner`. Returns `None` when no
/// resize is needed.
fn compute_target_inner(
    current_inner: (f64, f64),
    current_outer: (f64, f64),
    work: (f64, f64),
    min_inner: (f64, f64),
) -> Option<(f64, f64)> {
    let frame_w = (current_outer.0 - current_inner.0).max(0.0);
    let frame_h = (current_outer.1 - current_inner.1).max(0.0);

    let max_inner_w = (work.0 - frame_w).max(min_inner.0);
    let max_inner_h = (work.1 - frame_h).max(min_inner.1);

    let new_w = current_inner.0.min(max_inner_w).max(min_inner.0);
    let new_h = current_inner.1.min(max_inner_h).max(min_inner.1);

    if (new_w - current_inner.0).abs() > 0.5 || (new_h - current_inner.1).abs() > 0.5 {
        Some((new_w, new_h))
    } else {
        None
    }
}

#[cfg(windows)]
fn work_area_logical(
    window: &WebviewWindow,
    scale: f64,
) -> tauri::Result<Option<(f64, f64)>> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let tauri_hwnd = window.hwnd()?;
    let hwnd: HWND = tauri_hwnd.0;

    // SAFETY: `hwnd` is a valid top-level window handle obtained from Tauri.
    // MonitorFromWindow returns NULL only if the handle is invalid, which we
    // guard below before calling GetMonitorInfoW.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Ok(None);
    }

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: `info` is a valid, writable MONITORINFO with cbSize set, which
    // is the documented contract for GetMonitorInfoW.
    let ok = unsafe { GetMonitorInfoW(monitor, &mut info) };
    if ok == 0 {
        return Ok(None);
    }

    let width_px = f64::from(info.rcWork.right - info.rcWork.left);
    let height_px = f64::from(info.rcWork.bottom - info.rcWork.top);
    Ok(Some((width_px / scale, height_px / scale)))
}

#[cfg(not(windows))]
fn work_area_logical(
    window: &WebviewWindow,
    scale: f64,
) -> tauri::Result<Option<(f64, f64)>> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(None);
    };
    let size = monitor.size();
    let width = f64::from(size.width) / scale;
    let height = (f64::from(size.height) / scale) - NON_WINDOWS_HEIGHT_BUFFER;
    Ok(Some((width, height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: (f64, f64) = (MIN_WIDTH, MIN_HEIGHT);
    const FRAME_W: f64 = 16.0;
    const FRAME_H: f64 = 31.0;

    fn outer_of(inner: (f64, f64)) -> (f64, f64) {
        (inner.0 + FRAME_W, inner.1 + FRAME_H)
    }

    #[test]
    fn returns_none_when_already_fits() {
        let inner = (1280.0, 760.0);
        let result = compute_target_inner(inner, outer_of(inner), (1920.0, 1040.0), MIN);
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_when_small_window_fits_small_work_area() {
        let inner = (960.0, 600.0);
        let result = compute_target_inner(inner, outer_of(inner), (1366.0, 720.0), MIN);
        assert_eq!(result, None);
    }

    #[test]
    fn shrinks_height_when_taskbar_eats_vertical_space() {
        let inner = (1280.0, 760.0);
        let result = compute_target_inner(inner, outer_of(inner), (1366.0, 720.0), MIN);
        // work_h=720, frame_h=31 → target inner_h = 689
        assert_eq!(result, Some((1280.0, 689.0)));
    }

    #[test]
    fn shrinks_width_when_work_area_is_narrow() {
        let inner = (1280.0, 760.0);
        let result = compute_target_inner(inner, outer_of(inner), (1000.0, 900.0), MIN);
        // work_w=1000, frame_w=16 → target inner_w = 984
        assert_eq!(result, Some((984.0, 760.0)));
    }

    #[test]
    fn shrinks_both_dimensions_when_both_exceed() {
        let inner = (1280.0, 760.0);
        let result = compute_target_inner(inner, outer_of(inner), (1000.0, 720.0), MIN);
        assert_eq!(result, Some((984.0, 689.0)));
    }

    #[test]
    fn clamps_up_to_min_when_work_area_is_smaller_than_min() {
        let inner = (1280.0, 760.0);
        // work area is absurdly small; inner must stay at MIN.
        let result = compute_target_inner(inner, outer_of(inner), (800.0, 500.0), MIN);
        assert_eq!(result, Some((MIN_WIDTH, MIN_HEIGHT)));
    }

    #[test]
    fn honors_min_when_current_is_below_min_and_work_area_is_tiny() {
        // Degenerate case: window somehow starts below MIN. We still clamp
        // *up* to MIN regardless of work area.
        let inner = (800.0, 500.0);
        let result = compute_target_inner(inner, outer_of(inner), (900.0, 600.0), MIN);
        assert_eq!(result, Some((MIN_WIDTH, MIN_HEIGHT)));
    }

    #[test]
    fn handles_zero_frame_delta() {
        // Borderless/fullscreen-ish scenario: outer == inner.
        let inner = (1280.0, 760.0);
        let result = compute_target_inner(inner, inner, (1000.0, 600.0), MIN);
        assert_eq!(result, Some((1000.0, 600.0)));
    }

    #[test]
    fn subpixel_differences_do_not_trigger_resize() {
        let inner = (1280.0, 760.0);
        // Work area is 0.3px smaller than current outer — below the 0.5
        // threshold, so no resize.
        let work = (1280.0 + FRAME_W - 0.3, 760.0 + FRAME_H - 0.3);
        let result = compute_target_inner(inner, outer_of(inner), work, MIN);
        assert_eq!(result, None);
    }
}
