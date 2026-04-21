#![allow(dead_code)] // wired up incrementally through upcoming tasks

use rusqlite::{params, Connection, Row};
use std::collections::HashMap;

use crate::error::AppError;
use crate::tags::{Tag, TagCategory, TagSource};

fn row_to_tag(row: &Row) -> rusqlite::Result<Tag> {
    let category_str: String = row.get("category")?;
    let source_str: String = row.get("source")?;
    let category = TagCategory::from_str(&category_str).unwrap_or(TagCategory::User);
    let source = TagSource::from_str(&source_str).unwrap_or(TagSource::User);
    Ok(Tag {
        id: Some(row.get("id")?),
        segment_id: row.get("segment_id")?,
        trip_id: row.get("trip_id")?,
        name: row.get("name")?,
        category,
        source,
        scan_id: row.get("scan_id")?,
        scan_version: row.get::<_, Option<i64>>("scan_version")?.map(|v| v as u32),
        confidence: row.get::<_, Option<f64>>("confidence")?.map(|v| v as f32),
        start_ms: row.get("start_ms")?,
        end_ms: row.get("end_ms")?,
        note: row.get("note")?,
        metadata_json: row.get("metadata_json")?,
        created_ms: row.get("created_ms")?,
    })
}

pub fn insert_tag(conn: &Connection, tag: &Tag) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO tags (segment_id, trip_id, name, category, source, scan_id, scan_version,
                           confidence, start_ms, end_ms, note, metadata_json, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            tag.segment_id,
            tag.trip_id,
            tag.name,
            tag.category.as_str(),
            tag.source.as_str(),
            tag.scan_id,
            tag.scan_version.map(|v| v as i64),
            tag.confidence.map(|v| v as f64),
            tag.start_ms,
            tag.end_ms,
            tag.note,
            tag.metadata_json,
            tag.created_ms,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Delete all tags from a specific scan on a specific segment. Used to
/// replace scan output atomically before inserting fresh tags on re-run.
pub fn delete_scan_tags_for_segment(
    conn: &Connection,
    segment_id: &str,
    scan_id: &str,
) -> Result<usize, AppError> {
    let n = conn.execute(
        "DELETE FROM tags WHERE segment_id = ?1 AND scan_id = ?2",
        params![segment_id, scan_id],
    )?;
    Ok(n)
}

/// Replace the scan's output for a segment in a single transaction, and
/// record the scan_run.
#[allow(clippy::too_many_arguments)]
pub fn commit_scan_run(
    conn: &mut Connection,
    segment_id: &str,
    scan_id: &str,
    version: u32,
    status: &str,
    error_message: Option<&str>,
    tags: &[Tag],
    now_ms: i64,
) -> Result<(), AppError> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM tags WHERE segment_id = ?1 AND scan_id = ?2",
        params![segment_id, scan_id],
    )?;
    for tag in tags {
        tx.execute(
            "INSERT INTO tags (segment_id, trip_id, name, category, source, scan_id, scan_version,
                               confidence, start_ms, end_ms, note, metadata_json, created_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                tag.segment_id,
                tag.trip_id,
                tag.name,
                tag.category.as_str(),
                tag.source.as_str(),
                tag.scan_id,
                tag.scan_version.map(|v| v as i64),
                tag.confidence.map(|v| v as f64),
                tag.start_ms,
                tag.end_ms,
                tag.note,
                tag.metadata_json,
                tag.created_ms,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO scan_runs (segment_id, scan_id, version, ran_at_ms, status, error_message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(segment_id, scan_id) DO UPDATE SET
            version = excluded.version,
            ran_at_ms = excluded.ran_at_ms,
            status = excluded.status,
            error_message = excluded.error_message",
        params![segment_id, scan_id, version as i64, now_ms, status, error_message],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn all_tags(conn: &Connection) -> Result<Vec<Tag>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, segment_id, trip_id, name, category, source, scan_id, scan_version,
                confidence, start_ms, end_ms, note, metadata_json, created_ms
         FROM tags ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_tag)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn insert_user_tag_for_segments(
    conn: &mut Connection,
    segment_ids: &[String],
    name: &str,
    note: Option<&str>,
) -> Result<usize, AppError> {
    let category = crate::tags::vocabulary::builtin_category(name)
        .unwrap_or(crate::tags::TagCategory::User);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let tx = conn.transaction()?;
    let mut inserted = 0;
    for seg_id in segment_ids {
        // Dedup: if this exact user tag already exists on the segment,
        // skip rather than stacking duplicates.
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM tags WHERE segment_id = ?1 AND name = ?2 AND source = 'user' LIMIT 1",
                params![seg_id, name],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if exists {
            continue;
        }
        tx.execute(
            "INSERT INTO tags (segment_id, trip_id, name, category, source, scan_id, scan_version,
                               confidence, start_ms, end_ms, note, metadata_json, created_ms)
             VALUES (?1, NULL, ?2, ?3, 'user', NULL, NULL, NULL, NULL, NULL, ?4, NULL, ?5)",
            params![seg_id, name, category.as_str(), note, now_ms],
        )?;
        inserted += 1;
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn remove_user_tag_for_segments(
    conn: &mut Connection,
    segment_ids: &[String],
    name: &str,
) -> Result<usize, AppError> {
    let tx = conn.transaction()?;
    let mut removed = 0;
    for seg_id in segment_ids {
        removed += tx.execute(
            "DELETE FROM tags WHERE segment_id = ?1 AND name = ?2 AND source = 'user'",
            params![seg_id, name],
        )?;
    }
    tx.commit()?;
    Ok(removed)
}

pub fn tags_for_segment(conn: &Connection, segment_id: &str) -> Result<Vec<Tag>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, segment_id, trip_id, name, category, source, scan_id, scan_version,
                confidence, start_ms, end_ms, note, metadata_json, created_ms
         FROM tags WHERE segment_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![segment_id], row_to_tag)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn tags_for_trip(conn: &Connection, trip_id: &str) -> Result<Vec<Tag>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, segment_id, trip_id, name, category, source, scan_id, scan_version,
                confidence, start_ms, end_ms, note, metadata_json, created_ms
         FROM tags
         WHERE trip_id = ?1
            OR segment_id IN (SELECT id FROM segments WHERE trip_id = ?1)
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![trip_id], row_to_tag)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Library-wide map: trip_id -> (tag_name -> segment_count). Powers
/// sidebar aggregation badges without N+1 queries.
pub fn all_trip_tag_counts(conn: &Connection) -> Result<HashMap<String, HashMap<String, i64>>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT segments.trip_id, tags.name, COUNT(DISTINCT tags.segment_id) AS n
         FROM tags
         INNER JOIN segments ON segments.id = tags.segment_id
         GROUP BY segments.trip_id, tags.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for row in rows {
        let (trip_id, name, n) = row?;
        out.entry(trip_id).or_default().insert(name, n);
    }
    Ok(out)
}

/// Count of segments-with-each-tag-name within one trip. Used for sidebar
/// aggregation badges.
pub fn tag_counts_for_trip(conn: &Connection, trip_id: &str) -> Result<HashMap<String, i64>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT name, COUNT(DISTINCT segment_id) AS n
         FROM tags
         WHERE segment_id IN (SELECT id FROM segments WHERE trip_id = ?1)
         GROUP BY name",
    )?;
    let rows = stmt.query_map(params![trip_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (name, n) = row?;
        out.insert(name, n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::tags::Tag;

    fn make_tag(segment_id: &str, name: &str, scan_id: &str, source: TagSource) -> Tag {
        Tag {
            id: None,
            segment_id: Some(segment_id.to_string()),
            trip_id: None,
            name: name.to_string(),
            category: TagCategory::Motion,
            source,
            scan_id: if source == TagSource::User { None } else { Some(scan_id.to_string()) },
            scan_version: if source == TagSource::User { None } else { Some(1) },
            confidence: None,
            start_ms: None,
            end_ms: None,
            note: None,
            metadata_json: None,
            created_ms: 1_000,
        }
    }

    #[test]
    fn insert_and_query_roundtrip() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        insert_tag(&conn, &make_tag("seg-a", "stationary", "gps_stationary", TagSource::System)).unwrap();
        insert_tag(&conn, &make_tag("seg-a", "silent", "audio_rms", TagSource::System)).unwrap();
        let got = tags_for_segment(&conn, "seg-a").unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|t| t.name == "stationary"));
        assert!(got.iter().any(|t| t.name == "silent"));
    }

    #[test]
    fn delete_by_scan_leaves_user_tags() {
        let db = open_in_memory().unwrap();
        let mut conn = db.lock().unwrap();
        insert_tag(&conn, &make_tag("seg-a", "stationary", "gps_stationary", TagSource::System)).unwrap();
        insert_tag(&conn, &make_tag("seg-a", "keep", "", TagSource::User)).unwrap();

        // Re-running the GPS scan should nuke its tag but preserve the user's `keep`.
        commit_scan_run(
            &mut conn,
            "seg-a",
            "gps_stationary",
            2,
            "ok",
            None,
            &[],
            5_000,
        ).unwrap();

        let got = tags_for_segment(&conn, "seg-a").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "keep");
        assert_eq!(got[0].source, TagSource::User);
    }

    #[test]
    fn tag_counts_for_trip_groups_by_name() {
        let db = open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        // Set up segments belonging to a trip.
        conn.execute(
            "INSERT INTO segments (id, trip_id, start_time_ms, duration_s, master_path, is_event, last_seen_ms)
             VALUES ('seg-a', 'trip-x', 0, 60.0, '/a', 0, 100), ('seg-b', 'trip-x', 60000, 60.0, '/b', 0, 100)",
            [],
        ).unwrap();
        insert_tag(&conn, &make_tag("seg-a", "stationary", "gps_stationary", TagSource::System)).unwrap();
        insert_tag(&conn, &make_tag("seg-b", "stationary", "gps_stationary", TagSource::System)).unwrap();
        insert_tag(&conn, &make_tag("seg-a", "silent", "audio_rms", TagSource::System)).unwrap();

        let counts = tag_counts_for_trip(&conn, "trip-x").unwrap();
        assert_eq!(counts.get("stationary"), Some(&2));
        assert_eq!(counts.get("silent"), Some(&1));
    }
}
