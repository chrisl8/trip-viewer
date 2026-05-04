use serde::{Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid filename: {0}")]
    InvalidFilename(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("not a supported video file: {0}")]
    NotVideo(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("import already running")]
    ImportAlreadyRunning,

    #[error("no import in progress")]
    NoImportRunning,

    #[error("database error: {0}")]
    Db(String),

    /// A path was passed where it must live under the active archive root,
    /// but it doesn't. Surfaced during scans/imports and during the legacy
    /// migration when a stored absolute path can't be relativized.
    #[error("path is outside the active archive: {path}")]
    PathOutsideArchive { path: String },

    /// The archive's SQLite `user_version` is higher than what this build
    /// of Trip Viewer knows how to apply. Almost always means the user
    /// opened the archive on a newer release, then came back to an older
    /// one. Refuse rather than silently risk corruption.
    #[error("archive schema is newer than this app supports (found {found}, this build expects ≤ {expected})")]
    ArchiveSchemaTooNew { found: i32, expected: i32 },

    /// A command that operates on the active archive was invoked while
    /// no archive is open. The frontend's empty state catches this and
    /// prompts the user to open an archive.
    #[error("no archive is currently open")]
    NoArchiveOpen,
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::Db(err.to_string())
    }
}

impl From<rusqlite_migration::Error> for AppError {
    fn from(err: rusqlite_migration::Error) -> Self {
        AppError::Db(format!("migration: {err}"))
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
