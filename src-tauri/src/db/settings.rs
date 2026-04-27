//! Key/value settings table. Used for persistent app-level state that
//! isn't tied to a specific trip or segment: the user-configured ffmpeg
//! path, cached library root, feature-gate flags, etc.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>, AppError> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO settings (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at_ms = excluded.updated_at_ms",
        params![key, value, now],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn delete(conn: &Connection, key: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn set_and_get_roundtrip() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        set(&conn, "ffmpeg_path", "C:/ffmpeg/bin/ffmpeg.exe").unwrap();
        let got = get(&conn, "ffmpeg_path").unwrap();
        assert_eq!(got.as_deref(), Some("C:/ffmpeg/bin/ffmpeg.exe"));
    }

    #[test]
    fn missing_key_returns_none() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        assert!(get(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn set_overwrites() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        set(&conn, "k", "v1").unwrap();
        set(&conn, "k", "v2").unwrap();
        assert_eq!(get(&conn, "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn delete_removes_key() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        set(&conn, "k", "v").unwrap();
        delete(&conn, "k").unwrap();
        assert!(get(&conn, "k").unwrap().is_none());
    }
}
