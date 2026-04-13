use crate::error::AppError;
use std::path::Path;

/// Get free disk space in bytes for the volume containing `path`.
#[cfg(windows)]
pub fn free_disk_space(path: &Path) -> Result<u64, AppError> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide_path = to_wide_null(&path.to_string_lossy());
    let mut free_bytes_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;

    let result = unsafe {
        GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut free_bytes_available,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };

    if result == 0 {
        return Err(AppError::Internal(format!(
            "GetDiskFreeSpaceExW failed for {}",
            path.display()
        )));
    }

    Ok(free_bytes_available)
}

#[cfg(not(windows))]
pub fn free_disk_space(_path: &Path) -> Result<u64, AppError> {
    Err(AppError::Internal(
        "disk space check not supported on this platform".to_string(),
    ))
}

/// Convert a string to a null-terminated UTF-16 slice for Windows API calls.
#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Format bytes as a human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    const KB: u64 = 1 << 10;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
    }

    #[cfg(windows)]
    #[test]
    fn test_free_disk_space_current_dir() {
        let free = free_disk_space(Path::new("C:\\")).unwrap();
        assert!(free > 0, "Expected positive free space, got {free}");
    }
}
