use crate::error::AppError;
use chrono::Local;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) struct ImportLogger {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl ImportLogger {
    /// Create a new logger writing to `<logs_dir>/dashcam-YYYY-MM-DD_HHMMSS.log`.
    pub fn new(logs_dir: &Path) -> Result<Self, AppError> {
        fs::create_dir_all(logs_dir)?;

        let filename = format!(
            "dashcam-{}.log",
            Local::now().format("%Y-%m-%d_%H%M%S")
        );
        let path = logs_dir.join(&filename);
        let file = File::create(&path)?;
        let mut logger = Self {
            writer: BufWriter::with_capacity(64 * 1024, file),
            path,
        };
        logger.info("Log started");
        Ok(logger)
    }

    pub fn info(&mut self, msg: &str) {
        self.write_line("INFO", msg);
    }

    pub fn warn(&mut self, msg: &str) {
        self.write_line("WARN", msg);
    }

    pub fn error(&mut self, msg: &str) {
        self.write_line("ERROR", msg);
    }

    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_line(&mut self, level: &str, msg: &str) {
        let ts = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
        let _ = writeln!(self.writer, "{ts} [{level}] {msg}");
    }

    /// Delete logs older than `max_age` from the logs directory.
    pub fn rotate(logs_dir: &Path, max_age: Duration) {
        let entries = match fs::read_dir(logs_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        let cutoff = std::time::SystemTime::now() - max_age;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("dashcam-") || !name.ends_with(".log") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

impl Drop for ImportLogger {
    fn drop(&mut self) {
        self.info("Log ended");
        self.flush();
    }
}
