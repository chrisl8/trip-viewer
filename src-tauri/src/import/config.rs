use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const CONFIG_FILENAME: &str = ".import-config.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportConfig {
    #[serde(default)]
    pub ignored_extensions: Vec<String>,
    #[serde(default)]
    pub ignored_filenames: Vec<String>,
}

impl ImportConfig {
    /// Load config from `<root>/.import-config.json`. Returns default if missing.
    pub fn load(root_path: &Path) -> Self {
        let path = root_path.join(CONFIG_FILENAME);
        match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save config to `<root>/.import-config.json`.
    pub fn save(&self, root_path: &Path) -> Result<(), AppError> {
        let path = root_path.join(CONFIG_FILENAME);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        fs::write(&path, json)?;
        Ok(())
    }

    /// Check if a filename should be ignored (by extension or filename pattern).
    pub fn is_ignored(&self, filename: &str) -> bool {
        let lower = filename.to_lowercase();
        let ext = Path::new(&lower)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();

        // Check extensions
        if self
            .ignored_extensions
            .iter()
            .any(|e| e.to_lowercase() == ext)
        {
            return true;
        }

        // Check filenames (exact match, case-insensitive)
        if self
            .ignored_filenames
            .iter()
            .any(|f| f.eq_ignore_ascii_case(filename))
        {
            return true;
        }

        false
    }

    /// Add an extension to the ignored list and save.
    pub fn add_ignored_extension(
        &mut self,
        ext: &str,
        root_path: &Path,
    ) -> Result<(), AppError> {
        let ext_lower = ext.to_lowercase();
        if !self
            .ignored_extensions
            .iter()
            .any(|e| e.to_lowercase() == ext_lower)
        {
            self.ignored_extensions.push(ext_lower);
            self.save(root_path)?;
        }
        Ok(())
    }

    /// Add a filename to the ignored list and save.
    pub fn add_ignored_filename(
        &mut self,
        name: &str,
        root_path: &Path,
    ) -> Result<(), AppError> {
        if !self
            .ignored_filenames
            .iter()
            .any(|f| f.eq_ignore_ascii_case(name))
        {
            self.ignored_filenames.push(name.to_string());
            self.save(root_path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = ImportConfig::default();
        assert!(cfg.ignored_extensions.is_empty());
        assert!(cfg.ignored_filenames.is_empty());
    }

    #[test]
    fn test_is_ignored_extension() {
        let cfg = ImportConfig {
            ignored_extensions: vec![".thm".to_string(), ".tmp".to_string()],
            ignored_filenames: vec![],
        };
        assert!(cfg.is_ignored("video.THM"));
        assert!(cfg.is_ignored("data.tmp"));
        assert!(!cfg.is_ignored("video.mp4"));
    }

    #[test]
    fn test_is_ignored_filename() {
        let cfg = ImportConfig {
            ignored_extensions: vec![],
            ignored_filenames: vec!["thumbs.db".to_string(), "desktop.ini".to_string()],
        };
        assert!(cfg.is_ignored("Thumbs.db"));
        assert!(cfg.is_ignored("DESKTOP.INI"));
        assert!(!cfg.is_ignored("video.mp4"));
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ImportConfig::default();
        cfg.add_ignored_extension(".thm", dir.path()).unwrap();
        cfg.add_ignored_filename("thumbs.db", dir.path()).unwrap();

        let loaded = ImportConfig::load(dir.path());
        assert_eq!(loaded.ignored_extensions, vec![".thm"]);
        assert_eq!(loaded.ignored_filenames, vec!["thumbs.db"]);
    }

    #[test]
    fn test_no_duplicate_additions() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ImportConfig::default();
        cfg.add_ignored_extension(".thm", dir.path()).unwrap();
        cfg.add_ignored_extension(".THM", dir.path()).unwrap();
        assert_eq!(cfg.ignored_extensions.len(), 1);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ImportConfig::load(dir.path());
        assert!(cfg.ignored_extensions.is_empty());
    }
}
