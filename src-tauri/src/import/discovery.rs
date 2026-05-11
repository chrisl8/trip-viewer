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

#[cfg(target_os = "linux")]
pub fn find_sd_cards() -> Result<Vec<ImportSource>, AppError> {
    Ok(scan_linux_mount_roots(&linux_mount_roots()))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn find_sd_cards() -> Result<Vec<ImportSource>, AppError> {
    Ok(Vec::new())
}

/// Build the list of directories under which a removable drive's mount
/// point is likely to appear on Linux. Order matters for the first-write
/// dedup pass: `/run/media/<user>` comes first because udisks2 (the
/// modern default on Fedora, Arch, openSUSE, etc.) mounts there.
#[cfg(target_os = "linux")]
fn linux_mount_roots() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            roots.push(PathBuf::from(format!("/run/media/{user}")));
            roots.push(PathBuf::from(format!("/media/{user}")));
        }
    }

    // `/media` catches direct-mounted drives (some setups) and `/mnt`
    // catches manual mounts. Both are scanned shallowly — `detect_dashcam_kind`
    // rejects anything that doesn't have the brand folder signature.
    roots.push(PathBuf::from("/media"));
    roots.push(PathBuf::from("/mnt"));

    // Also scan sibling user dirs under `/run/media`, in case the process
    // is running under a different uname (e.g. flatpak portal) than the
    // user who owns the GUI session.
    roots.push(PathBuf::from("/run/media"));

    roots
}

#[cfg(target_os = "linux")]
fn scan_linux_mount_roots(roots: &[std::path::PathBuf]) -> Vec<ImportSource> {
    use std::collections::HashSet;
    use std::path::PathBuf;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut sources: Vec<ImportSource> = Vec::new();

    for parent in roots {
        let entries = match fs::read_dir(parent) {
            Ok(e) => e,
            Err(_) => continue, // missing root is normal; skip silently
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let child = entry.path();
            if !entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
                continue;
            }

            // Canonicalize to dedup. If the path can't be canonicalized
            // (e.g. permission denied), fall back to the raw path so the
            // card isn't silently dropped.
            let key = fs::canonicalize(&child).unwrap_or_else(|_| child.clone());
            if !seen.insert(key.clone()) {
                continue;
            }

            let kind = match detect_dashcam_kind(&child) {
                Some(k) => k,
                None => continue,
            };

            let label = child
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "sd".to_string());
            let (file_count, total_bytes) = count_files_and_size(&child);

            sources.push(ImportSource {
                path: key.to_string_lossy().to_string(),
                label,
                read_only: !is_writable(&child),
                file_count,
                total_bytes,
                detected_kind: Some(kind),
            });
        }
    }

    sources
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

    #[cfg(target_os = "linux")]
    #[test]
    fn test_scan_linux_mount_roots_finds_wolfbox() {
        // Simulate /run/media/<user>/ with one Wolf Box card and one
        // unrelated drive (e.g. an NTFS data disk).
        let parent = tempfile::tempdir().unwrap();
        let card = parent.path().join("disk");
        fs::create_dir(&card).unwrap();
        for f in &["front_norm", "rear_norm", "extra_norm"] {
            fs::create_dir(card.join(f)).unwrap();
        }
        let data = parent.path().join("Matrix");
        fs::create_dir(&data).unwrap();
        fs::create_dir(data.join("Documents")).unwrap();

        let sources = scan_linux_mount_roots(&[parent.path().to_path_buf()]);
        assert_eq!(sources.len(), 1, "only the Wolf Box card should be returned");
        assert_eq!(sources[0].label, "disk");
        assert_eq!(sources[0].detected_kind, Some(CameraKind::WolfBox));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_scan_linux_mount_roots_dedups_across_parents() {
        // Same card visible via two parent dirs (e.g. /run/media/user AND
        // /media/user, which some hybrid setups create) should only be
        // returned once.
        let p1 = tempfile::tempdir().unwrap();
        let card = p1.path().join("disk");
        fs::create_dir(&card).unwrap();
        for f in &["front_norm", "rear_norm", "extra_norm"] {
            fs::create_dir(card.join(f)).unwrap();
        }
        // p2 is a symlink to p1, simulating the same physical mount
        // appearing under two parents.
        let p2_dir = tempfile::tempdir().unwrap();
        let p2_link = p2_dir.path().join("link");
        std::os::unix::fs::symlink(p1.path(), &p2_link).unwrap();

        let sources = scan_linux_mount_roots(&[p1.path().to_path_buf(), p2_link]);
        assert_eq!(sources.len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_scan_linux_mount_roots_missing_parent_is_ok() {
        let sources =
            scan_linux_mount_roots(&[std::path::PathBuf::from("/definitely/does/not/exist")]);
        assert!(sources.is_empty());
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
