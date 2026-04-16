use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Accepted video container extensions. `.mp4` covers Wolf Box and Thinkware;
/// `.mov` covers Miltona (QuickTime container, still readable by the `mp4`
/// crate since QuickTime is ISO-BMFF's ancestor).
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov"];

pub fn find_video_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
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
}
