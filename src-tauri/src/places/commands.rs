//! Tauri commands for managing the user's saved places. Creating,
//! editing, or deleting a place makes the `gps_place` scan's existing
//! output stale — the user can press Scan (new-only or rescan-stale
//! scope) after modifying places to refresh tags. Deletion cascades
//! via `db::places::delete_place`, which removes any `place_<id>` tag
//! rows in the same transaction.

use tauri::State;

use crate::db::places::{self, Place};
use crate::db::DbHandle;
use crate::error::AppError;

#[tauri::command]
pub async fn list_places(db: State<'_, DbHandle>) -> Result<Vec<Place>, AppError> {
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    places::list_places(&conn)
}

#[tauri::command]
pub async fn add_place(
    name: String,
    lat: f64,
    lon: f64,
    radius_m: f64,
    db: State<'_, DbHandle>,
) -> Result<i64, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::Internal("place name is empty".into()));
    }
    if radius_m <= 0.0 {
        return Err(AppError::Internal("radius must be positive".into()));
    }
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    places::insert_place(&conn, name.trim(), lat, lon, radius_m)
}

#[tauri::command]
pub async fn update_place(
    id: i64,
    name: String,
    lat: f64,
    lon: f64,
    radius_m: f64,
    db: State<'_, DbHandle>,
) -> Result<(), AppError> {
    if name.trim().is_empty() {
        return Err(AppError::Internal("place name is empty".into()));
    }
    if radius_m <= 0.0 {
        return Err(AppError::Internal("radius must be positive".into()));
    }
    let conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    places::update_place(&conn, id, name.trim(), lat, lon, radius_m)
}

#[tauri::command]
pub async fn delete_place(id: i64, db: State<'_, DbHandle>) -> Result<(), AppError> {
    let mut conn = db.lock().map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    places::delete_place(&mut conn, id)
}
