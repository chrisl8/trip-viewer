pub mod decoder;

use crate::error::AppError;
use crate::model::{GpsBatchItem, GpsPoint};
use rayon::prelude::*;
use std::path::Path;

#[tauri::command]
pub async fn extract_gps(path: String) -> Result<Vec<GpsPoint>, AppError> {
    decoder::extract(Path::new(&path))
}

#[tauri::command]
pub async fn extract_gps_batch(paths: Vec<String>) -> Result<Vec<GpsBatchItem>, AppError> {
    let results: Vec<GpsBatchItem> = paths
        .par_iter()
        .map(|p| {
            let points = decoder::extract(Path::new(p)).unwrap_or_default();
            GpsBatchItem {
                file_path: p.clone(),
                points,
            }
        })
        .collect();
    Ok(results)
}
