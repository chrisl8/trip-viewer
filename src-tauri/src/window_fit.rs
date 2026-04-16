//! Clamp the main window to the monitor's usable work area on startup so the
//! default size from `tauri.conf.json` never leaves the transport controls
//! hidden behind the OS taskbar on small laptop displays (issue #5).

use tauri::{LogicalSize, WebviewWindow};

const MIN_WIDTH: f64 = 960.0;
const MIN_HEIGHT: f64 = 600.0;
#[cfg(not(windows))]
const NON_WINDOWS_HEIGHT_BUFFER: f64 = 80.0;

pub fn fit_to_work_area(window: &WebviewWindow) -> tauri::Result<()> {
    let scale = window.scale_factor()?;

    if let Some((work_w, work_h)) = work_area_logical(window, scale)? {
        let outer = window.outer_size()?;
        let current_w = f64::from(outer.width) / scale;
        let current_h = f64::from(outer.height) / scale;
        let new_w = current_w.min(work_w).max(MIN_WIDTH);
        let new_h = current_h.min(work_h).max(MIN_HEIGHT);

        if (new_w - current_w).abs() > 0.5 || (new_h - current_h).abs() > 0.5 {
            window.set_size(LogicalSize::new(new_w, new_h))?;
        }
    }

    window.center()?;
    window.show()?;
    Ok(())
}

#[cfg(windows)]
fn work_area_logical(
    _window: &WebviewWindow,
    scale: f64,
) -> tauri::Result<Option<(f64, f64)>> {
    use std::ptr::addr_of_mut;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};

    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `rect` is a valid, writable RECT on the stack. SPI_GETWORKAREA
    // ignores uiparam/fwinini and fills the pvparam buffer with the primary
    // monitor's work rect (taskbar excluded).
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, addr_of_mut!(rect).cast(), 0)
    };
    if ok == 0 {
        return Ok(None);
    }
    let width_px = f64::from(rect.right - rect.left);
    let height_px = f64::from(rect.bottom - rect.top);
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
