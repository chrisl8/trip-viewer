use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::error::AppError;

/// Migration head — bumped every time a new `.sql` file is added to
/// `migrations/`. Compared against `PRAGMA user_version` on open so we
/// can refuse archives written by a newer Trip Viewer (the schema
/// version would be ahead of what this build knows how to apply).
pub const HEAD_VERSION: u32 = 13;

const M0001: &str = include_str!("migrations/0001_init.sql");
const M0002: &str = include_str!("migrations/0002_places.sql");
const M0003: &str = include_str!("migrations/0003_timelapse_jobs.sql");
const M0004: &str = include_str!("migrations/0004_settings.sql");
const M0005: &str = include_str!("migrations/0005_padded_count.sql");
const M0006: &str = include_str!("migrations/0006_speed_curve.sql");
const M0007: &str = include_str!("migrations/0007_trip_camera_meta.sql");
const M0008: &str = include_str!("migrations/0008_manual_trip_merges.sql");
const M0009: &str = include_str!("migrations/0009_storage_sizes.sql");
const M0010: &str = include_str!("migrations/0010_segment_tombstones.sql");
const M0011: &str = include_str!("migrations/0011_drop_settings_table.sql");
const M0012: &str = include_str!("migrations/0012_trip_gps.sql");
const M0013: &str = include_str!("migrations/0013_timelapse_relative_paths.sql");

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(M0001),
        M::up(M0002),
        M::up(M0003),
        M::up(M0004),
        M::up(M0005),
        M::up(M0006),
        M::up(M0007),
        M::up(M0008),
        M::up(M0009),
        M::up(M0010),
        M::up(M0011),
        M::up(M0012),
        M::up(M0013),
    ])
}

pub fn apply(conn: &mut Connection) -> Result<(), AppError> {
    migrations().to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the absolute → archive-relative rewrite in
    /// `0013_timelapse_relative_paths.sql`. The migration must
    /// preserve the *filename* portion of the existing `output_path`
    /// (which encodes the on-disk trip_id at encode time) and only
    /// rewrite the directory prefix. An earlier draft of this
    /// migration re-derived the filename from the row's current
    /// columns — that destroyed the mapping for archives whose
    /// trip_id had been remapped by `rebuild_for_cross_os` while the
    /// on-disk files kept their pre-remap names.
    #[test]
    fn migration_0013_preserves_filename_when_normalizing_paths() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO trips (id, start_time_ms, end_time_ms, camera_kind,
                gps_supported, last_seen_ms)
             VALUES ('current-trip', 0, 60000, 'wolfBox', 1, 0)",
            [],
        )
        .unwrap();
        // Scenario A: a row whose absolute output_path contains an
        // OLD trip_id (different from the row's current trip_id —
        // the kind of state a cross-OS UUID rewrite leaves behind).
        // The migration must keep the OLD trip_id in the filename;
        // only the directory prefix changes.
        let old_filename_uuid = "97f9e7f8-f72e-5db1-9462-752187d17da3";
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms)
             VALUES ('current-trip', '8x', 'F', 'done',
                ?1,
                1000)",
            rusqlite::params![format!(
                "/run/media/old-mount/Wolfbox Dashcam/Timelapses/{old_filename_uuid}_8x_F.mp4"
            )],
        )
        .unwrap();
        // Scenario B: a row whose path is ALREADY relative (e.g. a
        // DB written after the relativization fix landed). Migration
        // should be a near no-op — extract the basename and keep
        // the same directory prefix.
        let stable_uuid = "11111111-2222-3333-4444-555555555555";
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms)
             VALUES ('current-trip', '16x', 'F', 'done',
                ?1,
                1000)",
            rusqlite::params![format!("Timelapses/{stable_uuid}_16x_F.mp4")],
        )
        .unwrap();
        // Scenario C: a pending row with NULL output_path. Must stay NULL.
        conn.execute(
            "INSERT INTO timelapse_jobs
                (trip_id, tier, channel, status, output_path, created_at_ms)
             VALUES ('current-trip', '8x', 'I', 'pending', NULL, 1000)",
            [],
        )
        .unwrap();

        conn.execute_batch(M0013).unwrap();

        // Scenario A: stored filename keeps the OLD UUID, not 'current-trip'.
        let a: String = conn
            .query_row(
                "SELECT output_path FROM timelapse_jobs
                 WHERE trip_id = 'current-trip' AND tier = '8x' AND channel = 'F'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            a,
            format!("Timelapses/{old_filename_uuid}_8x_F.mp4"),
            "filename portion must be preserved, not re-derived from row.trip_id"
        );

        // Scenario B: round-trip preserves the value.
        let b: String = conn
            .query_row(
                "SELECT output_path FROM timelapse_jobs
                 WHERE trip_id = 'current-trip' AND tier = '16x' AND channel = 'F'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b, format!("Timelapses/{stable_uuid}_16x_F.mp4"));

        // Scenario C: NULL stays NULL.
        let c: Option<String> = conn
            .query_row(
                "SELECT output_path FROM timelapse_jobs
                 WHERE trip_id = 'current-trip' AND tier = '8x' AND channel = 'I'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(c.is_none(), "NULL output_path must stay NULL");
    }

    #[test]
    fn migrations_apply_cleanly() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        // Settings table is dropped by 0011 — per-machine keys live in
        // app_data_dir/settings.json now.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('segments','trips','tags','scan_runs','places','timelapse_jobs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 6);
        let has_settings: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_settings, 0, "settings table should be dropped by 0011");
    }
}
