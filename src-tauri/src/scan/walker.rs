use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

/// Accepted video container extensions. `.mp4` covers Wolf Box and Thinkware;
/// `.mov` covers Miltona (QuickTime container, still readable by the `mp4`
/// crate since QuickTime is ISO-BMFF's ancestor).
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov"];

/// Decide whether to descend into a directory during the scan walk.
/// Skips:
/// - `Timelapses/` — our own pre-render output, written by
///   `timelapse::worker::resolve_output_root`. Filenames there are
///   `<trip_id>_<tier>_<channel>.mp4` and have no business being
///   parsed by the scanner.
/// - Any directory whose name starts with `.` — by convention these
///   are scratch / staging dirs (e.g. import's `.staging/`, the
///   timelapse worker's `.tmp/`) that should never appear as input.
///
/// Never skips the root the user pointed at, even if it happens to
/// match one of those rules — they explicitly asked us to look there.
fn should_skip_dir(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    if entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    name == "Timelapses" || name.starts_with('.')
}

pub fn find_video_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip_dir(e))
        .filter_map(|e| e.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let path = entry.into_path();
            match path.extension().and_then(|e| e.to_str()) {
                Some(ext)
                    if VIDEO_EXTENSIONS
                        .iter()
                        .any(|v| ext.eq_ignore_ascii_case(v)) =>
                {
                    Some(path)
                }
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_mp4_and_mov_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.mp4"), b"x").unwrap();
        fs::write(dir.path().join("b.MP4"), b"x").unwrap();
        fs::write(dir.path().join("c.mov"), b"x").unwrap();
        fs::write(dir.path().join("d.MOV"), b"x").unwrap();
        fs::write(dir.path().join("skip.avi"), b"x").unwrap();
        fs::write(dir.path().join("skip.txt"), b"x").unwrap();

        let found = find_video_files(dir.path());
        assert_eq!(found.len(), 4, "should pick up both .mp4 and .mov (any case)");
    }

    #[test]
    fn recurses_into_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("deep.MOV"), b"x").unwrap();

        let found = find_video_files(dir.path());
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn skips_timelapses_output_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Videos.mp4"), b"x").unwrap();
        fs::create_dir(dir.path().join("Timelapses")).unwrap();
        fs::write(
            dir.path().join("Timelapses").join("trip_8x_F.mp4"),
            b"x",
        )
        .unwrap();
        fs::write(
            dir.path().join("Timelapses").join("trip_16x_F.mp4"),
            b"x",
        )
        .unwrap();

        let found = find_video_files(dir.path());
        assert_eq!(
            found.len(),
            1,
            "Timelapses/ outputs should not be returned as scan input"
        );
        assert!(found[0].file_name().unwrap() == "Videos.mp4");
    }

    #[test]
    fn skips_dot_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real.mp4"), b"x").unwrap();
        fs::create_dir(dir.path().join(".staging")).unwrap();
        fs::write(dir.path().join(".staging").join("scratch.mp4"), b"x").unwrap();

        let found = find_video_files(dir.path());
        assert_eq!(found.len(), 1);
    }
}
