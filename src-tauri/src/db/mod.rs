use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::AppError;

mod migrations;
pub mod places;
pub mod segments;
pub mod tags;

pub type DbHandle = Arc<Mutex<Connection>>;

pub fn open(db_path: &Path) -> Result<DbHandle, AppError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::apply(&mut conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

#[cfg(test)]
#[allow(dead_code)]
pub fn open_in_memory() -> Result<DbHandle, AppError> {
    let mut conn = Connection::open_in_memory()?;
    migrations::apply(&mut conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}
