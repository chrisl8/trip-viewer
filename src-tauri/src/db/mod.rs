use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;

use crate::error::AppError;

pub mod manual_trip_merges;
mod migrations;
pub mod places;
pub mod segments;
pub mod settings;
pub mod tags;
pub mod timelapse_jobs;
pub mod trip_gps;

/// Bundles a per-archive SQLite connection with the archive root path it
/// applies to. Path columns in the DB are stored *relative* to this
/// root; all conversion goes through `crate::paths`. Callers continue to
/// use `db.lock()` exactly as before — the wrapper passes through to the
/// inner mutex via `Deref`-like accessor on the Arc.
pub struct DbHandleInner {
    conn: Mutex<Connection>,
    archive_root: PathBuf,
}

impl DbHandleInner {
    pub fn lock(&self) -> std::sync::LockResult<MutexGuard<'_, Connection>> {
        self.conn.lock()
    }

    pub fn archive_root(&self) -> &Path {
        &self.archive_root
    }
}

pub type DbHandle = Arc<DbHandleInner>;

/// Open the per-archive SQLite DB at `<archive_root>/.tripviewer/tripviewer.db`.
///
/// Not yet wired into startup — the legacy flow still calls
/// `open_at_path` to keep using `app_data_dir/tripviewer.db`. The
/// per-archive migration that's coming next switches startup to this
/// entry point once the user has confirmed an archive root.
#[allow(dead_code)]
/// Creates the directory if needed, applies migrations to head, and
/// returns a handle bundling the connection with the archive root.
///
/// Refuses to open an archive whose `user_version` is newer than the
/// migration head this build knows about (`AppError::ArchiveSchemaTooNew`)
/// — almost always means the user opened the archive with a newer Trip
/// Viewer release and is now back on an older one.
pub fn open(archive_root: &Path) -> Result<DbHandle, AppError> {
    let archive_root = dunce::canonicalize(archive_root).map_err(|e| {
        AppError::Internal(format!(
            "canonicalize archive root {}: {e}",
            archive_root.display()
        ))
    })?;
    let db_dir = archive_root.join(".tripviewer");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("tripviewer.db");
    open_at_path(&db_path, &archive_root)
}

/// Lower-level open that takes an explicit DB file path *and* the
/// archive root the file should be associated with. Used by the
/// transitional startup flow that still opens the legacy DB at
/// `app_data_dir/tripviewer.db` until migration_v2 ships, and by tests.
/// New code should call `open(archive_root)` instead so the file path
/// stays in lockstep with the per-archive convention.
pub fn open_at_path(db_path: &Path, archive_root: &Path) -> Result<DbHandle, AppError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(db_path)?;

    // Refuse to open archives whose schema is newer than what this
    // build can handle. Without this check, rusqlite_migration would
    // silently succeed (no-op when current >= target) but our code
    // would then try to read columns it doesn't know about.
    let current_user_version: i32 =
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let expected = migrations::HEAD_VERSION as i32;
    if current_user_version > expected {
        return Err(AppError::ArchiveSchemaTooNew {
            found: current_user_version,
            expected,
        });
    }

    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::apply(&mut conn)?;

    Ok(Arc::new(DbHandleInner {
        conn: Mutex::new(conn),
        archive_root: archive_root.to_path_buf(),
    }))
}

#[cfg(test)]
#[allow(dead_code)]
pub fn open_in_memory() -> Result<DbHandle, AppError> {
    open_in_memory_with_root(Path::new("/tmp/tripviewer-test-archive"))
}

#[cfg(test)]
#[allow(dead_code)]
pub fn open_in_memory_with_root(archive_root: &Path) -> Result<DbHandle, AppError> {
    let mut conn = Connection::open_in_memory()?;
    // Mirror open_at_path so FK cascades (e.g. trip_gps → trips) behave
    // identically in tests and production.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::apply(&mut conn)?;
    Ok(Arc::new(DbHandleInner {
        conn: Mutex::new(conn),
        archive_root: archive_root.to_path_buf(),
    }))
}
