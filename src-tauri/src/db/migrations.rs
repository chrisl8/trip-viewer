use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::error::AppError;

const M0001: &str = include_str!("migrations/0001_init.sql");
const M0002: &str = include_str!("migrations/0002_places.sql");
const M0003: &str = include_str!("migrations/0003_timelapse_jobs.sql");
const M0004: &str = include_str!("migrations/0004_settings.sql");
const M0005: &str = include_str!("migrations/0005_padded_count.sql");
const M0006: &str = include_str!("migrations/0006_speed_curve.sql");
const M0007: &str = include_str!("migrations/0007_trip_camera_meta.sql");
const M0008: &str = include_str!("migrations/0008_manual_trip_merges.sql");

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
    ])
}

pub fn apply(conn: &mut Connection) -> Result<(), AppError> {
    migrations().to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_cleanly() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('segments','trips','tags','scan_runs','places','timelapse_jobs','settings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 7);
    }
}
