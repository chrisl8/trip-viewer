//! Per-machine settings persisted as JSON in `app_data_dir/settings.json`.
//!
//! Replaces the SQLite `settings` table for keys that describe the *machine*
//! Trip Viewer is running on (ffmpeg binary location, encoder capability cache,
//! worker concurrency override) rather than the *archive* it's looking at.
//! Splitting these out lets the per-archive DB (PR 2) travel with the user's
//! video drive without dragging Windows-specific paths along for the ride.
//!
//! On first launch after upgrade, `migrate_from_sqlite` reads the four legacy
//! keys out of the SQLite settings table, populates this struct, deletes them
//! from SQLite, and bumps `schema_version` to 1 so the migration is idempotent.
//!
//! `recent_archives` and `last_archive` are reserved for PR 2/3 (multi-archive
//! support); they're in the schema now so we don't need a v2 bump later.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

const SETTINGS_FILENAME: &str = "settings.json";

/// On-disk schema version of `settings.json` itself. v1 added the
/// per-machine keys extracted from the SQLite settings table.
///
/// The cross-OS DB-rewrite tracks separately in `cross_os_migrated`
/// because it depends on having an archive open, and an archive may
/// be added or replaced after the JSON is at v1.
const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub ffmpeg_path: Option<String>,
    #[serde(default)]
    pub ffmpeg_version: Option<String>,
    #[serde(default)]
    pub nvenc_hevc: Option<bool>,
    #[serde(default)]
    pub timelapse_max_concurrent_jobs: Option<u32>,
    #[serde(default)]
    pub recent_archives: Vec<RecentArchive>,
    #[serde(default)]
    pub last_archive: Option<String>,
    /// Set once per-archive after the cross-OS DB rewrite (segment UUIDs
    /// rebuilt from archive-relative paths, `master_path` columns
    /// relativized). Maps archive root path → done. Keyed by path
    /// rather than baked into a single boolean so a user with multiple
    /// archives can have each migrate independently.
    #[serde(default)]
    pub cross_os_migrated_archives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentArchive {
    pub path: String,
    pub label: String,
    pub last_opened_ms: i64,
}

/// Tauri-managed handle that owns the on-disk path and a synchronized
/// snapshot of the settings. Reads clone the struct (it's small); writes
/// take an exclusive lock, mutate, and atomically save in one operation.
pub struct AppSettingsHandle {
    inner: RwLock<AppSettings>,
    path: PathBuf,
}

impl AppSettingsHandle {
    /// Load `app_data_dir/settings.json`. Returns a handle wrapping the
    /// parsed contents, or a default-initialized struct if the file is
    /// missing or unparseable. A parse failure is logged to stderr but
    /// doesn't block startup — the user gets a fresh-defaults experience
    /// rather than a hard crash on a corrupted config.
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join(SETTINGS_FILENAME);
        let settings = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str::<AppSettings>(&s).unwrap_or_else(|e| {
                eprintln!(
                    "[app_settings] failed to parse {}: {e} — falling back to defaults",
                    path.display()
                );
                AppSettings::default()
            }),
            Err(_) => AppSettings::default(),
        };
        Self {
            inner: RwLock::new(settings),
            path,
        }
    }

    /// Snapshot of current settings. Cheap — clone is just four Options
    /// plus a small Vec.
    pub fn read(&self) -> AppSettings {
        self.inner
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Apply a mutation under the write lock and persist to disk
    /// atomically before releasing the lock. If the write fails the
    /// in-memory state still reflects the change — the next successful
    /// save will flush it. Returns the error so the caller can decide
    /// whether to surface it.
    pub fn update<F>(&self, f: F) -> Result<(), AppError>
    where
        F: FnOnce(&mut AppSettings),
    {
        let mut guard = self
            .inner
            .write()
            .map_err(|_| AppError::Internal("app_settings lock poisoned".into()))?;
        f(&mut guard);
        save_atomic(&self.path, &guard)
    }
}

/// Write `settings` to `path` using a temp-file + rename so a crash
/// mid-write doesn't leave a zero-byte or truncated file. This is a
/// stricter contract than `import/config.rs` offers — justified because
/// settings.json gates app startup behavior, while the import config
/// can tolerate a corrupted save (load returns default).
fn save_atomic(path: &Path, settings: &AppSettings) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| AppError::Internal(format!("serialize settings: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// One-time migration from the SQLite `settings` table to JSON. Idempotent:
/// once `schema_version >= CURRENT_SCHEMA_VERSION`, this is a no-op.
///
/// Reads the four per-machine keys, populates the in-memory settings,
/// and saves the JSON file. The legacy SQLite rows are left in place —
/// the per-archive migration backs up the legacy DB whole-cloth, so a
/// targeted DELETE here would be wasted work. Operating only as a
/// reader also lets us run against a read-only connection, which the
/// per-archive migration uses for safety.
pub fn migrate_from_sqlite(
    handle: &AppSettingsHandle,
    db: &rusqlite::Connection,
) -> Result<(), AppError> {
    {
        let current = handle.read();
        if current.schema_version >= CURRENT_SCHEMA_VERSION {
            return Ok(());
        }
    }

    let ffmpeg_path = crate::db::settings::get(db, "ffmpeg_path")?;
    let ffmpeg_version = crate::db::settings::get(db, "ffmpeg_version")?;
    let nvenc_hevc = crate::db::settings::get(db, "nvenc_hevc")?.map(|s| s == "1" || s == "true");
    let max_concurrent = crate::db::settings::get(db, "timelapse_max_concurrent_jobs")?
        .and_then(|s| s.parse::<u32>().ok());

    handle.update(|s| {
        // Don't clobber a value the user has already set in the JSON
        // (defense in depth — the schema_version gate above already
        // prevents a second run, but if someone hand-edits the JSON to
        // lower the version, at least we won't lose their config).
        if s.ffmpeg_path.is_none() {
            s.ffmpeg_path = ffmpeg_path;
        }
        if s.ffmpeg_version.is_none() {
            s.ffmpeg_version = ffmpeg_version;
        }
        if s.nvenc_hevc.is_none() {
            s.nvenc_hevc = nvenc_hevc;
        }
        if s.timelapse_max_concurrent_jobs.is_none() {
            s.timelapse_max_concurrent_jobs = max_concurrent;
        }
        s.schema_version = CURRENT_SCHEMA_VERSION;
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Build a connection with the legacy schema shape (settings table
    /// present) so the migration tests have something to read out of.
    /// Production fresh-archive DBs no longer carry the settings table
    /// — see migration 0011.
    fn legacy_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        let s = h.read();
        assert_eq!(s.schema_version, 0);
        assert!(s.ffmpeg_path.is_none());
        assert!(s.recent_archives.is_empty());
    }

    #[test]
    fn load_corrupt_returns_default() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(SETTINGS_FILENAME), "{not json").unwrap();
        let h = AppSettingsHandle::load(dir.path());
        assert_eq!(h.read().schema_version, 0);
    }

    #[test]
    fn update_roundtrips_through_disk() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        h.update(|s| {
            s.ffmpeg_path = Some("/usr/bin/ffmpeg".into());
            s.nvenc_hevc = Some(true);
            s.schema_version = 1;
        })
        .unwrap();
        let h2 = AppSettingsHandle::load(dir.path());
        let s = h2.read();
        assert_eq!(s.ffmpeg_path.as_deref(), Some("/usr/bin/ffmpeg"));
        assert_eq!(s.nvenc_hevc, Some(true));
        assert_eq!(s.schema_version, 1);
    }

    #[test]
    fn save_atomic_does_not_leave_tmp_on_success() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        h.update(|s| s.schema_version = 1).unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        assert!(entries.iter().any(|n| n == SETTINGS_FILENAME));
        assert!(!entries
            .iter()
            .any(|n| n.to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn migration_pulls_keys_from_sqlite() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        let conn = legacy_conn();
        crate::db::settings::set(&conn, "ffmpeg_path", "/usr/bin/ffmpeg").unwrap();
        crate::db::settings::set(&conn, "ffmpeg_version", "8.1").unwrap();
        crate::db::settings::set(&conn, "nvenc_hevc", "1").unwrap();
        crate::db::settings::set(&conn, "timelapse_max_concurrent_jobs", "4").unwrap();
        // library_root is per-archive scope, not per-machine; the
        // per-archive migration handles it separately.
        crate::db::settings::set(&conn, "library_root", "/some/path").unwrap();

        migrate_from_sqlite(&h, &conn).unwrap();

        let s = h.read();
        assert_eq!(s.ffmpeg_path.as_deref(), Some("/usr/bin/ffmpeg"));
        assert_eq!(s.ffmpeg_version.as_deref(), Some("8.1"));
        assert_eq!(s.nvenc_hevc, Some(true));
        assert_eq!(s.timelapse_max_concurrent_jobs, Some(4));
        assert_eq!(s.schema_version, CURRENT_SCHEMA_VERSION);

        // Legacy SQLite rows are *not* deleted — the per-archive
        // migration backs up the legacy DB whole-cloth, so we don't
        // bother with targeted cleanup here. Keeping this read-only
        // also lets us run against a read-only connection.
        assert_eq!(
            crate::db::settings::get(&conn, "ffmpeg_path")
                .unwrap()
                .as_deref(),
            Some("/usr/bin/ffmpeg")
        );
        assert_eq!(
            crate::db::settings::get(&conn, "library_root")
                .unwrap()
                .as_deref(),
            Some("/some/path")
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        let conn = legacy_conn();
        crate::db::settings::set(&conn, "ffmpeg_path", "/first").unwrap();
        migrate_from_sqlite(&h, &conn).unwrap();

        // Second call is a no-op even if SQLite is somehow re-populated.
        crate::db::settings::set(&conn, "ffmpeg_path", "/second").unwrap();
        migrate_from_sqlite(&h, &conn).unwrap();

        assert_eq!(h.read().ffmpeg_path.as_deref(), Some("/first"));
    }

    #[test]
    fn migration_with_empty_sqlite_still_bumps_version() {
        let dir = tempdir().unwrap();
        let h = AppSettingsHandle::load(dir.path());
        let conn = legacy_conn();
        migrate_from_sqlite(&h, &conn).unwrap();
        let s = h.read();
        assert_eq!(s.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(s.ffmpeg_path.is_none());
    }
}
