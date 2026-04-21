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
