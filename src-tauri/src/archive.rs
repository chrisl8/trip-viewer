//! Multi-archive runtime state and switching commands.
//!
//! Tauri-managed `ArchiveSlot` holds an `Option<DbHandle>`. Replacing
//! the inner value swaps which per-archive DB the rest of the app
//! reads/writes; setting it to `None` puts the app in the no-archive
//! empty state the frontend renders before the first folder is picked.
//!
//! Workers (timelapse, scans) hold `DbHandle` clones for the lifetime
//! of their run. On archive switch we signal cancel and swap the slot
//! immediately — outstanding workers finish writing to the *old*
//! archive's DB, which is fine: their `Arc<DbHandleInner>` keeps that
//! connection alive until they drop it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tauri::State;

use crate::app_settings::{AppSettingsHandle, RecentArchive};
use crate::db::{self, DbHandle};
use crate::error::AppError;

/// Tauri-managed handle that wraps the optional currently-open archive.
/// Cheap to clone; readers compete only on the inner `RwLock`. Most
/// command paths take a brief read lock to clone the inner `Arc`, then
/// drop the lock before doing DB work — keeps switching latency at
/// "the next read lock release" rather than "scan completes".
pub type ArchiveSlot = Arc<RwLock<Option<DbHandle>>>;

pub fn new_slot() -> ArchiveSlot {
    Arc::new(RwLock::new(None))
}

/// Pull the active `DbHandle` out of the slot or return
/// [`AppError::NoArchiveOpen`]. Every Tauri command that needs the DB
/// goes through here; the frontend's empty state handles the error.
pub fn require_db(slot: &ArchiveSlot) -> Result<DbHandle, AppError> {
    slot.read()
        .map_err(|_| AppError::Internal("archive slot poisoned".into()))?
        .as_ref()
        .cloned()
        .ok_or(AppError::NoArchiveOpen)
}

/// Replace the slot's contents. Used internally by `open_archive` and
/// `close_archive`; not exposed directly.
fn replace_slot(slot: &ArchiveSlot, value: Option<DbHandle>) -> Result<(), AppError> {
    let mut guard = slot
        .write()
        .map_err(|_| AppError::Internal("archive slot poisoned".into()))?;
    *guard = value;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentArchiveInfo {
    pub root: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentArchiveInfo {
    pub path: String,
    pub label: String,
    pub last_opened_ms: i64,
    /// True if the path is reachable on disk right now. Lets the
    /// frontend show offline archives in a muted state without
    /// removing them from the list.
    pub online: bool,
}

const RECENT_ARCHIVES_MAX: usize = 20;

/// Open the archive at `path` and install it into the slot. Replaces
/// any currently-open archive. Updates `last_archive` and
/// `recent_archives` in `settings.json`. Runs the per-archive
/// cross-OS rewrite if it hasn't run for this root yet.
#[tauri::command]
pub async fn open_archive(
    path: String,
    slot: State<'_, ArchiveSlot>,
    settings: State<'_, AppSettingsHandle>,
) -> Result<CurrentArchiveInfo, AppError> {
    let raw_root = PathBuf::from(&path);
    if !raw_root.exists() {
        return Err(AppError::Internal(format!(
            "archive folder does not exist: {}",
            raw_root.display()
        )));
    }
    let archive_root = dunce::canonicalize(&raw_root).map_err(|e| {
        AppError::Internal(format!(
            "canonicalize {}: {e}",
            raw_root.display()
        ))
    })?;

    let handle = db::open(&archive_root)?;
    if let Err(e) = crate::timelapse::cleanup::cleanup_stale_jobs(&handle) {
        eprintln!("[archive] timelapse cleanup failed: {e}");
    }
    if let Err(e) = crate::migration_v2::rebuild_for_cross_os(&handle, &settings) {
        eprintln!("[archive] cross-OS rewrite failed: {e}");
    }

    let info = build_current_info(&archive_root);
    replace_slot(&slot, Some(handle))?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let root_str = archive_root.to_string_lossy().into_owned();
    let label = info.label.clone();
    settings.update(|s| {
        s.last_archive = Some(root_str.clone());
        s.recent_archives.retain(|r| r.path != root_str);
        s.recent_archives.insert(
            0,
            RecentArchive {
                path: root_str.clone(),
                label,
                last_opened_ms: now_ms,
            },
        );
        if s.recent_archives.len() > RECENT_ARCHIVES_MAX {
            s.recent_archives.truncate(RECENT_ARCHIVES_MAX);
        }
    })?;

    Ok(info)
}

/// Drop the currently-open archive. The frontend's empty state takes
/// over. Persists the cleared `last_archive` so a relaunch doesn't
/// auto-reopen.
#[tauri::command]
pub async fn close_archive(
    slot: State<'_, ArchiveSlot>,
    settings: State<'_, AppSettingsHandle>,
) -> Result<(), AppError> {
    replace_slot(&slot, None)?;
    settings.update(|s| s.last_archive = None)?;
    Ok(())
}

/// Returns the currently-open archive, if any.
#[tauri::command]
pub async fn current_archive(
    slot: State<'_, ArchiveSlot>,
) -> Result<Option<CurrentArchiveInfo>, AppError> {
    let guard = slot
        .read()
        .map_err(|_| AppError::Internal("archive slot poisoned".into()))?;
    Ok(guard.as_ref().map(|h| build_current_info(h.archive_root())))
}

/// Recent archives, with each entry's online status checked by
/// stat'ing its path.
#[tauri::command]
pub async fn list_recent_archives(
    settings: State<'_, AppSettingsHandle>,
) -> Result<Vec<RecentArchiveInfo>, AppError> {
    let s = settings.read();
    Ok(s.recent_archives
        .iter()
        .map(|r| RecentArchiveInfo {
            path: r.path.clone(),
            label: r.label.clone(),
            last_opened_ms: r.last_opened_ms,
            online: Path::new(&r.path).is_dir(),
        })
        .collect())
}

/// Drop a path from the recent list. Doesn't touch the archive itself.
#[tauri::command]
pub async fn forget_archive(
    path: String,
    settings: State<'_, AppSettingsHandle>,
) -> Result<(), AppError> {
    settings.update(|s| {
        s.recent_archives.retain(|r| r.path != path);
    })?;
    Ok(())
}

fn build_current_info(archive_root: &Path) -> CurrentArchiveInfo {
    let label = archive_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| archive_root.display().to_string());
    CurrentArchiveInfo {
        root: archive_root.to_string_lossy().into_owned(),
        label,
    }
}
