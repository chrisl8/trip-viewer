use crate::error::AppError;
use crate::import::types::ImportSource;
use crate::scan::naming::CameraKind;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

/// Wolf Box dashcam folder names. A drive is a Wolf Box SD card if its
/// root contains at least `WOLFBOX_MIN_MATCH` of these.
const WOLFBOX_FOLDERS: &[&str] = &[
    "front_norm",
    "front_emer",
    "front_photo",
    "rear_norm",
    "rear_emer",
    "rear_photo",
    "extra_norm",
    "extra_emer",
    "extra_photo",
];
const WOLFBOX_MIN_MATCH: usize = 3;

/// Thinkware folder names. Only 4 candidates total so the threshold is
/// relaxed — users may not record in every mode (e.g. parking-only users
/// won't have `manual_rec`).
const THINKWARE_FOLDERS: &[&str] = &["cont_rec", "evt_rec", "manual_rec", "parking_rec"];
const THINKWARE_MIN_MATCH: usize = 2;

/// System directories to skip during file counting.
const SKIPPED_DIRS: &[&str] = &["system volume information", "$recycle.bin"];

pub(crate) fn is_skipped_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIPPED_DIRS.iter().any(|&s| s == lower)
}

/// Discover removable drives that look like dashcam SD cards. Recognizes
/// Wolf Box and Thinkware layouts; Miltona's folder structure is unknown
/// so those cards must be opened manually.
#[cfg(windows)]
pub fn find_sd_cards() -> Result<Vec<ImportSource>, AppError> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_REMOVABLE: u32 = 2;

    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(AppError::Internal("GetLogicalDrives failed".into()));
    }

    let mut sources = Vec::new();

    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }

        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide_root = to_wide_null(&root);

        let drive_type = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
        if drive_type != DRIVE_REMOVABLE {
            continue;
        }

        let detected_kind = match detect_dashcam_kind(Path::new(&root)) {
            Some(k) => k,
            None => continue,
        };

        let (file_count, total_bytes) = count_files_and_size(Path::new(&root));

        sources.push(ImportSource {
            path: root.clone(),
            label: format!("sd-{letter}"),
            read_only: !is_writable(Path::new(&root)),
            file_count,
            total_bytes,
            detected_kind: Some(detected_kind),
        });
    }

    Ok(sources)
}

#[cfg(not(windows))]
pub fn find_sd_cards() -> Result<Vec<ImportSource>, AppError> {
    Ok(Vec::new())
}

/// Return the dashcam brand whose folder signature matches this directory,
/// or `None` if it doesn't look like any supported SD card.
pub fn detect_dashcam_kind(path: &Path) -> Option<CameraKind> {
    let dir_names: Vec<String> = match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.metadata().map(|m| m.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_lowercase())
            .collect(),
        Err(_) => return None,
    };

    let wolfbox_count = WOLFBOX_FOLDERS
        .iter()
        .filter(|f| dir_names.iter().any(|d| d == **f))
        .count();
    if wolfbox_count >= WOLFBOX_MIN_MATCH {
        return Some(CameraKind::WolfBox);
    }

    let thinkware_count = THINKWARE_FOLDERS
        .iter()
        .filter(|f| dir_names.iter().any(|d| d == **f))
        .count();
    if thinkware_count >= THINKWARE_MIN_MATCH {
        return Some(CameraKind::Thinkware);
    }

    None
}

/// Test if a path is writable by creating and deleting a temp file.
pub fn is_writable(path: &Path) -> bool {
    let tmp = path.join(".dashcam-writetest");
    match fs::File::create(&tmp) {
        Ok(_) => {
            let _ = fs::remove_file(&tmp);
            true
        }
        Err(_) => false,
    }
}

/// Walk a source directory and count all files (excluding system dirs and PreAllocFiles).
fn count_files_and_size(root: &Path) -> (u32, u64) {
    let mut count = 0u32;
    let mut total = 0u64;

    for entry in WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        if e.file_type().is_dir() {
            !is_skipped_dir(&e.file_name().to_string_lossy())
        } else {
            true
        }
    }) {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            if name.starts_with(".PreAllocFile_") {
                continue;
            }
            count += 1;
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }

    (count, total)
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_dashcam_root_with_enough_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("front_norm")).unwrap();
        fs::create_dir(dir.path().join("rear_norm")).unwrap();
        fs::create_dir(dir.path().join("extra_norm")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), Some(CameraKind::WolfBox));
    }

    #[test]
    fn test_is_dashcam_root_not_enough_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("front_norm")).unwrap();
        fs::create_dir(dir.path().join("rear_norm")).unwrap();
        assert!(detect_dashcam_kind(dir.path()).is_none());
    }

    #[test]
    fn test_is_dashcam_root_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("FRONT_NORM")).unwrap();
        fs::create_dir(dir.path().join("Rear_Norm")).unwrap();
        fs::create_dir(dir.path().join("extra_PHOTO")).unwrap();
        assert!(detect_dashcam_kind(dir.path()).is_some());
    }

    #[test]
    fn test_thinkware_folder_signature_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cont_rec")).unwrap();
        fs::create_dir(dir.path().join("evt_rec")).unwrap();
        fs::create_dir(dir.path().join("manual_rec")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), Some(CameraKind::Thinkware));
    }

    #[test]
    fn test_thinkware_two_folder_minimum() {
        // Only `cont_rec` — below threshold.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cont_rec")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), None);

        // cont_rec + evt_rec — at threshold.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("cont_rec")).unwrap();
        fs::create_dir(dir.path().join("evt_rec")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), Some(CameraKind::Thinkware));
    }

    #[test]
    fn test_thinkware_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Cont_Rec")).unwrap();
        fs::create_dir(dir.path().join("EVT_REC")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), Some(CameraKind::Thinkware));
    }

    #[test]
    fn test_wolfbox_wins_over_thinkware_when_both_signatures_present() {
        // Pathological mixed drive — Wolf Box signature is stronger (3 folder
        // minimum vs 2), so it takes precedence when both are present.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("front_norm")).unwrap();
        fs::create_dir(dir.path().join("rear_norm")).unwrap();
        fs::create_dir(dir.path().join("extra_norm")).unwrap();
        fs::create_dir(dir.path().join("cont_rec")).unwrap();
        fs::create_dir(dir.path().join("evt_rec")).unwrap();
        assert_eq!(detect_dashcam_kind(dir.path()), Some(CameraKind::WolfBox));
    }

    #[test]
    fn test_is_writable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_writable(dir.path()));
    }

    #[test]
    fn test_is_skipped_dir() {
        assert!(is_skipped_dir("System Volume Information"));
        assert!(is_skipped_dir("$Recycle.Bin"));
        assert!(is_skipped_dir("$RECYCLE.BIN"));
        assert!(!is_skipped_dir("front_norm"));
    }

    #[test]
    fn test_count_files_and_size() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("front_norm")).unwrap();
        fs::write(dir.path().join("front_norm").join("test.mp4"), "hello").unwrap();
        fs::write(dir.path().join("front_norm").join("test2.mp4"), "world!").unwrap();

        let (count, size) = count_files_and_size(dir.path());
        assert_eq!(count, 2);
        assert_eq!(size, 11); // "hello" (5) + "world!" (6)
    }
}
