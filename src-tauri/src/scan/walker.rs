use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn find_mp4_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let path = entry.into_path();
            match path.extension().and_then(|e| e.to_str()) {
                Some(ext) if ext.eq_ignore_ascii_case("mp4") => Some(path),
                _ => None,
            }
        })
        .collect()
}
