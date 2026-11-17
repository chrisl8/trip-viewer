use std::path::Path;
use std::time::UNIX_EPOCH;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::archive::{require_db, ArchiveSlot};
use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaHealthRange {
    pub path: String,
    pub start_s: f64,
    pub end_s: Option<f64>,
    pub error_kind: String,
}

pub(crate) fn fingerprint(path: &Path) -> Result<(i64, i64), AppError> {
    let meta = std::fs::metadata(path)?;
    let size = i64::try_from(meta.len())
        .map_err(|_| AppError::Internal("media file size exceeds i64".into()))?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok((size, modified_ms))
}

pub(crate) fn stored_path(path: &Path, archive_root: &Path) -> String {
    crate::paths::to_archive_relative(path, archive_root)
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn load_media_health(
    paths: Vec<String>,
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<MediaHealthRange>, AppError> {
    let db = require_db(&slot)?;
    let archive_root = db.archive_root().to_path_buf();
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;

    let mut out = Vec::new();
    for original in paths {
        out.extend(load_file_health(&conn, &original, &archive_root)?);
    }
    Ok(out)
}

fn load_file_health(
    conn: &rusqlite::Connection,
    original: &str,
    archive_root: &Path,
) -> Result<Vec<MediaHealthRange>, AppError> {
    let path = Path::new(original);
    let Ok((size, modified_ms)) = fingerprint(path) else {
        return Ok(Vec::new());
    };
    let stored = stored_path(path, archive_root);
    let mut stmt = conn.prepare(
        "SELECT start_ms, end_ms, error_kind FROM media_health_ranges
         WHERE file_path = ?1 AND file_size = ?2 AND modified_ms = ?3 ORDER BY start_ms ASC",
    )?;
    let rows = stmt.query_map(params![stored, size, modified_ms], |r| {
        Ok(MediaHealthRange {
            path: original.to_string(),
            start_s: r.get::<_, i64>(0)? as f64 / 1000.0,
            end_s: r.get::<_, Option<i64>>(1)?.map(|v| v as f64 / 1000.0),
            error_kind: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[tauri::command]
pub async fn record_media_failure(
    path: String,
    start_s: f64,
    error_kind: String,
    slot: State<'_, ArchiveSlot>,
) -> Result<(), AppError> {
    if !start_s.is_finite() || start_s < 0.0 {
        return Ok(());
    }

    let db = require_db(&slot)?;
    let archive_root = db.archive_root().to_path_buf();
    let path_obj = Path::new(&path);
    let (size, modified_ms) = fingerprint(path_obj)?;
    let stored = stored_path(path_obj, &archive_root);
    let start_ms = (start_s * 1000.0).round() as i64;
    let now = chrono::Utc::now().timestamp_millis();

    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;

    persist_failure(
        &conn,
        &stored,
        size,
        modified_ms,
        start_ms,
        &error_kind,
        now,
    )
}

fn persist_failure(
    conn: &rusqlite::Connection,
    stored: &str,
    size: i64,
    modified_ms: i64,
    start_ms: i64,
    error_kind: &str,
    now: i64,
) -> Result<(), AppError> {
    let open_id: Option<i64> = conn
        .query_row(
            "SELECT id
             FROM media_health_ranges
             WHERE file_path = ?1 AND file_size = ?2 AND modified_ms = ?3
               AND end_ms IS NULL
             ORDER BY start_ms DESC
             LIMIT 1",
            params![stored, size, modified_ms],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = open_id {
        conn.execute(
            "UPDATE media_health_ranges
             SET start_ms = MIN(start_ms, ?2),
                 error_kind = ?3,
                 updated_at_ms = ?4
             WHERE id = ?1",
            params![id, start_ms, error_kind, now],
        )?;
    } else {
        conn.execute(
            "INSERT INTO media_health_ranges
                (file_path, file_size, modified_ms, start_ms, end_ms, error_kind, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
            params![stored, size, modified_ms, start_ms, error_kind, now],
        )?;
    }
    Ok(())
}

#[tauri::command]
pub async fn record_media_recovery(
    path: String,
    end_s: f64,
    slot: State<'_, ArchiveSlot>,
) -> Result<(), AppError> {
    if !end_s.is_finite() || end_s < 0.0 {
        return Ok(());
    }

    let db = require_db(&slot)?;
    let archive_root = db.archive_root().to_path_buf();
    let path_obj = Path::new(&path);
    let (size, modified_ms) = fingerprint(path_obj)?;
    let stored = stored_path(path_obj, &archive_root);
    let end_ms = (end_s * 1000.0).round() as i64;
    let now = chrono::Utc::now().timestamp_millis();

    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;

    persist_recovery(&conn, &stored, size, modified_ms, end_ms, now)
}

fn persist_recovery(
    conn: &rusqlite::Connection,
    stored: &str,
    size: i64,
    modified_ms: i64,
    end_ms: i64,
    now: i64,
) -> Result<(), AppError> {
    let open: Option<(i64, i64)> = conn
        .query_row(
            "SELECT id, start_ms
             FROM media_health_ranges
             WHERE file_path = ?1 AND file_size = ?2 AND modified_ms = ?3
               AND end_ms IS NULL AND start_ms <= ?4
             ORDER BY start_ms DESC
             LIMIT 1",
            params![stored, size, modified_ms, end_ms],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    if let Some((id, start_ms)) = open {
        conn.execute(
            "UPDATE media_health_ranges
             SET end_ms = ?2, updated_at_ms = ?3
             WHERE id = ?1",
            params![id, end_ms.max(start_ms), now],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_camera_ranges_close_persist_and_invalidate_on_replacement() {
        let root = tempfile::tempdir().unwrap();
        let db = crate::db::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        for name in ["front.mp4", "rear.mp4"] {
            let path = root.path().join(name);
            std::fs::write(&path, b"media").unwrap();
            let (size, modified) = fingerprint(&path).unwrap();
            persist_failure(&conn, name, size, modified, 89000, "decode", 1).unwrap();
            persist_failure(&conn, name, size, modified, 90000, "decode", 2).unwrap();
            let loaded = load_file_health(&conn, path.to_str().unwrap(), root.path()).unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].start_s, 89.0);
            assert_eq!(loaded[0].end_s, None);
        }
        let front = root.path().join("front.mp4");
        let (size, modified) = fingerprint(&front).unwrap();
        // A backward seek to a healthy frame must not close a later failure.
        persist_recovery(&conn, "front.mp4", size, modified, 10000, 3).unwrap();
        assert_eq!(
            load_file_health(&conn, front.to_str().unwrap(), root.path()).unwrap()[0].end_s,
            None
        );
        persist_recovery(&conn, "front.mp4", size, modified, 100000, 4).unwrap();
        assert_eq!(
            load_file_health(&conn, front.to_str().unwrap(), root.path()).unwrap()[0].end_s,
            Some(100.0)
        );
        let rear = root.path().join("rear.mp4");
        assert_eq!(
            load_file_health(&conn, rear.to_str().unwrap(), root.path()).unwrap()[0].end_s,
            None
        );
        std::fs::write(&front, b"repaired, different size").unwrap();
        assert!(
            load_file_health(&conn, front.to_str().unwrap(), root.path())
                .unwrap()
                .is_empty()
        );
        let (size, modified) = fingerprint(&rear).unwrap();
        persist_recovery(&conn, "rear.mp4", size, modified, 110000, 5).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&rear)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(123)),
            )
            .unwrap();
        assert!(load_file_health(&conn, rear.to_str().unwrap(), root.path())
            .unwrap()
            .is_empty());
    }
}
