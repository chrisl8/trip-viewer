//! Tauri commands backing the IssuesView — operations on flagged files.
//!
//! v1 action: move-to-trash. Uses the `trash` crate so the file lands in
//! the OS recycle bin (Windows) / Trash (macOS) / freedesktop.org trash
//! (Linux), recoverable from there for the usual OS-level retention
//! window. Never permanently deletes.

use crate::error::AppError;
use std::path::Path;

#[tauri::command]
pub fn issues_delete_to_trash(path: String) -> Result<(), AppError> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(AppError::Internal(format!(
            "file no longer exists: {}",
            p.display()
        )));
    }
    trash::delete(p).map_err(|e| AppError::Internal(format!("trash::delete failed: {e}")))
}
