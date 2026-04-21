//! GPS extraction — dispatches to a brand-specific decoder based on
//! `CameraKind` since each dashcam stores GPS in its own proprietary layout.

pub mod miltona;
pub mod shenshu;

use crate::error::AppError;
use crate::model::{GpsBatchItem, GpsPoint};
use crate::scan::naming::CameraKind;
use rayon::prelude::*;
use serde::Deserialize;
use std::path::Path;

/// A single path plus the camera brand the scanner identified for it. The
/// frontend builds one of these per segment (by pairing each master channel's
/// file path with its segment's `cameraKind`) and submits them in a batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsRequest {
    pub path: String,
    pub camera_kind: CameraKind,
}

#[tauri::command]
pub async fn extract_gps(path: String, camera_kind: CameraKind) -> Result<Vec<GpsPoint>, AppError> {
    extract_for_kind(Path::new(&path), camera_kind)
}

#[tauri::command]
pub async fn extract_gps_batch(
    requests: Vec<GpsRequest>,
) -> Result<Vec<GpsBatchItem>, AppError> {
    let results: Vec<GpsBatchItem> = requests
        .par_iter()
        .map(|req| {
            let points =
                extract_for_kind(Path::new(&req.path), req.camera_kind).unwrap_or_default();
            GpsBatchItem {
                file_path: req.path.clone(),
                points,
            }
        })
        .collect();
    Ok(results)
}

/// Write a diagnostic dump of a Miltona file's `gps0` atom. Used by the
/// "Export GPS debug" UI button to collect ground-truth samples while the
/// lat/lon encoding is still being finalized.
#[tauri::command]
pub async fn dump_miltona_gps_debug(path: String) -> Result<String, AppError> {
    let out = miltona::dump_debug(Path::new(&path))?;
    Ok(out.to_string_lossy().into_owned())
}

pub fn extract_for_kind(path: &Path, kind: CameraKind) -> Result<Vec<GpsPoint>, AppError> {
    match kind {
        CameraKind::WolfBox => shenshu::extract(path),
        CameraKind::Miltona => miltona::extract(path),
        // Thinkware: no GPS decoder (the sample we have contains no GPS
        // data at all). If a GPS-equipped Thinkware model turns up, add a
        // decoder and flip `CameraKind::gps_supported` for that variant.
        CameraKind::Thinkware => Ok(vec![]),
        // Generic fallback: try Wolf Box's decoder as a best-guess since
        // the ShenShu meta-track layout is the only one we know, but log
        // that we're guessing. Often this will just return an empty vec.
        CameraKind::Generic => shenshu::extract(path),
    }
}
