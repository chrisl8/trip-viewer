//! Disposable analysis sidecar. Never changes tripviewer.db or its schema.
use super::{DbHandle, DbHandleInner};
use crate::error::AppError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub mod trip_gps;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS trip_gps (
 trip_id TEXT PRIMARY KEY, points_json TEXT NOT NULL, point_count INTEGER NOT NULL,
 parser_version INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, source_signature TEXT
);
CREATE TABLE IF NOT EXISTS gps_segment_checkpoints (
 trip_id TEXT NOT NULL, segment_id TEXT NOT NULL, source_signature TEXT NOT NULL,
 parser_version INTEGER NOT NULL, payload_json TEXT NOT NULL,
 PRIMARY KEY (trip_id, segment_id)
);
CREATE TABLE IF NOT EXISTS media_health_ranges (
 id INTEGER PRIMARY KEY AUTOINCREMENT, file_path TEXT NOT NULL, file_size INTEGER NOT NULL,
 modified_ms INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER,
 error_kind TEXT NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_media_health_lookup
 ON media_health_ranges(file_path, file_size, modified_ms, start_ms);
PRAGMA user_version = 1;
"#;

pub fn open(root: &Path) -> Result<DbHandle, AppError> {
    let root = dunce::canonicalize(root)?;
    open_at_path(&root.join(".tripviewer/analysis-cache-v1.sqlite"), &root)
}

pub fn open_at_path(path: &Path, root: &Path) -> Result<DbHandle, AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    initialize(conn, root)
}

fn initialize(conn: Connection, root: &Path) -> Result<DbHandle, AppError> {
    conn.execute_batch(SCHEMA)?;
    Ok(Arc::new(DbHandleInner {
        conn: Mutex::new(conn),
        archive_root: root.to_path_buf(),
    }))
}

#[cfg(test)]
pub fn open_in_memory(root: &Path) -> Result<DbHandle, AppError> {
    initialize(Connection::open_in_memory()?, root)
}

/// Existing timelapse GPS remains usable when originals are gone. A legacy
/// row is not considered provenance-validated when originals still exist.
pub fn seed_legacy_trip(
    source: &DbHandle,
    cache: &DbHandle,
    trip_id: &str,
) -> Result<(), AppError> {
    let exists: bool = cache
        .lock()
        .map_err(|_| AppError::Internal("cache mutex poisoned".into()))?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM trip_gps WHERE trip_id=?1)",
            [trip_id],
            |r| r.get(0),
        )?;
    if exists {
        return Ok(());
    }
    let points = {
        let conn = source
            .lock()
            .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
        super::trip_gps::load(&conn, trip_id)?
    };
    if let Some(points) = points {
        let conn = cache
            .lock()
            .map_err(|_| AppError::Internal("cache mutex poisoned".into()))?;
        let payload = serde_json::to_string(&points)
            .map_err(|e| AppError::Internal(format!("legacy GPS cache: {e}")))?;
        conn.execute("INSERT OR IGNORE INTO trip_gps(trip_id,points_json,point_count,parser_version,created_at_ms) VALUES (?1,?2,?3,0,?4)",
    rusqlite::params![trip_id,payload,points.len() as i64,chrono::Utc::now().timestamp_millis()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_archived_track_seeds_sidecar_without_overwriting_an_existing_cache() {
        let root = tempfile::tempdir().unwrap();
        let source = crate::db::open(root.path()).unwrap();
        {
            let conn = source.lock().unwrap();
            conn.execute("INSERT INTO trips(id,start_time_ms,end_time_ms,camera_kind,gps_supported,last_seen_ms) VALUES ('trip',0,1000,'generic',1,0)",[]).unwrap();
            super::super::trip_gps::upsert(&conn, "trip", &[], 3).unwrap();
        }
        let cache = open(root.path()).unwrap();
        seed_legacy_trip(&source, &cache, "trip").unwrap();
        assert!(trip_gps::load(&cache.lock().unwrap(), "trip")
            .unwrap()
            .is_some());
        trip_gps::upsert(&cache.lock().unwrap(), "trip", &[], 8).unwrap();
        seed_legacy_trip(&source, &cache, "trip").unwrap();
        assert!(trip_gps::has_current(&cache.lock().unwrap(), "trip", 8).unwrap());
    }
    #[test]
    fn sidecar_does_not_change_archive_schema_or_require_upstream_migrations() {
        let root = tempfile::tempdir().unwrap();
        let source = crate::db::open(root.path()).unwrap();
        let snapshot = || {
            let conn = source.lock().unwrap();
            let version: i32 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            let mut stmt = conn
                .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")
                .unwrap();
            let schema = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (version, schema)
        };
        let before = snapshot();
        assert_eq!(before.0, 13);
        let cache = open(root.path()).unwrap();
        {
            let conn = cache.lock().unwrap();
            trip_gps::upsert(&conn, "trip", &[], 8).unwrap();
            crate::db::gps_segment_checkpoints::upsert(&conn, "trip", "segment", "source", 8, "{}")
                .unwrap();
            conn.execute("INSERT INTO media_health_ranges(file_path,file_size,modified_ms,start_ms,error_kind,updated_at_ms) VALUES ('front',1,1,0,'decode',0)",[]).unwrap();
        }
        assert_eq!(snapshot(), before);
        drop(source);
        assert!(crate::db::open(root.path()).is_ok());
        assert_eq!(crate::db::migrations::HEAD_VERSION, 13);
    }
}
