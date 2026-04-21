use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::error::AppError;

const M0001: &str = include_str!("migrations/0001_init.sql");
const M0002: &str = include_str!("migrations/0002_places.sql");

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(M0001), M::up(M0002)])
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
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('segments','trips','tags','scan_runs','places')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }
}
