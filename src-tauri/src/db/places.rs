use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: i64,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub radius_m: f64,
    pub created_ms: i64,
}

pub fn list_places(conn: &Connection) -> Result<Vec<Place>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, lat, lon, radius_m, created_ms FROM places ORDER BY created_ms ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Place {
            id: r.get(0)?,
            name: r.get(1)?,
            lat: r.get(2)?,
            lon: r.get(3)?,
            radius_m: r.get(4)?,
            created_ms: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn insert_place(
    conn: &Connection,
    name: &str,
    lat: f64,
    lon: f64,
    radius_m: f64,
) -> Result<i64, AppError> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO places (name, lat, lon, radius_m, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![name, lat, lon, radius_m, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_place(
    conn: &Connection,
    id: i64,
    name: &str,
    lat: f64,
    lon: f64,
    radius_m: f64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE places SET name = ?2, lat = ?3, lon = ?4, radius_m = ?5 WHERE id = ?1",
        params![id, name, lat, lon, radius_m],
    )?;
    Ok(())
}

/// Delete a place and any tags/scan_runs entries that reference it via
/// the `place_<id>` naming convention. Leaves other tags untouched.
pub fn delete_place(conn: &mut Connection, id: i64) -> Result<(), AppError> {
    let tag_name = format!("place_{id}");
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM tags WHERE name = ?1", params![tag_name])?;
    tx.execute("DELETE FROM places WHERE id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn insert_and_list_roundtrip() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let id = insert_place(&conn, "Home", 37.0, -122.0, 100.0).unwrap();
        assert!(id > 0);
        let places = list_places(&conn).unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "Home");
        assert_eq!(places[0].lat, 37.0);
    }

    #[test]
    fn update_changes_fields() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let id = insert_place(&conn, "Home", 37.0, -122.0, 100.0).unwrap();
        update_place(&conn, id, "Home (primary)", 37.1, -122.1, 200.0).unwrap();
        let places = list_places(&conn).unwrap();
        assert_eq!(places[0].name, "Home (primary)");
        assert_eq!(places[0].radius_m, 200.0);
    }

    #[test]
    fn delete_place_cascades_to_tags() {
        let db = open_in_memory().unwrap();
        let mut conn = db.lock().unwrap();
        let id = insert_place(&conn, "Home", 37.0, -122.0, 100.0).unwrap();

        // Seed a couple of tags, one for this place and one unrelated.
        conn.execute(
            "INSERT INTO tags (segment_id, name, category, source, created_ms)
             VALUES ('seg-a', ?1, 'place', 'system', 1000),
                    ('seg-a', 'silent', 'audio', 'system', 1000)",
            params![format!("place_{id}")],
        )
        .unwrap();

        delete_place(&mut conn, id).unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "only the unrelated tag should survive");
        let place_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM places", [], |r| r.get(0))
            .unwrap();
        assert_eq!(place_count, 0);
    }
}
