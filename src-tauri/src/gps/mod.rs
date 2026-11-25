//! GPS extraction — dispatches to a brand-specific decoder based on
//! `CameraKind` since each dashcam stores GPS in its own proprietary layout.

pub mod mai70;
pub mod miltona;
pub mod shenshu;
pub mod viofo;
mod viofo_timing;

use crate::archive::{require_db, ArchiveSlot};
use crate::error::AppError;
use crate::model::{GpsBatchItem, GpsPoint};
use crate::scan::naming::CameraKind;
use rayon::prelude::*;
use serde::Deserialize;
use std::path::Path;
use tauri::State;

/// Bump when shenshu/miltona decoders change semantics so previously
/// archived GPS becomes stale. The encoder's `has_current` probe and the
/// startup backfill both compare against this; rows below the current
/// version are re-extracted on the next encode (or backfill pass) when
/// the original MP4 is still on disk.
///
/// v2: trip-stitched GPS now trims each segment's points to the
/// segment's video duration. Parking-mode clips embed GPS for the whole
/// parked interval (~90 min) into a ~180s video; the untrimmed points
/// pushed later segments backwards in concat time, producing a
/// non-monotonic track that desynced the map from the video. All v1
/// rows must be re-stitched.
pub const GPS_PARSER_VERSION: i32 = 2;

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
pub async fn extract_gps_batch(requests: Vec<GpsRequest>) -> Result<Vec<GpsBatchItem>, AppError> {
    Ok(extract_batch(&requests))
}

enum Decoded {
    Other(Vec<GpsPoint>),
    Viofo(Vec<viofo::TimedGpsPoint>),
}

/// Keep input/descriptor order and validate adjacent VIOFO boundaries before
/// discarding UTC evidence. Used by live extraction and archived GPS stitching.
pub(crate) fn extract_batch(requests: &[GpsRequest]) -> Vec<GpsBatchItem> {
    extract_batch_with_other(requests, |path, kind| {
        extract_for_kind(path, kind).unwrap_or_default()
    })
}

fn extract_batch_with_other<F>(requests: &[GpsRequest], decode_other: F) -> Vec<GpsBatchItem>
where
    F: Fn(&Path, CameraKind) -> Vec<GpsPoint> + Sync,
{
    let mut decoded: Vec<Decoded> = requests
        .par_iter()
        .map(|req| {
            if req.camera_kind == CameraKind::Viofo {
                Decoded::Viofo(viofo::extract_video_timed(Path::new(&req.path)).unwrap_or_default())
            } else {
                Decoded::Other(decode_other(Path::new(&req.path), req.camera_kind))
            }
        })
        .collect();
    for index in 0..decoded.len().saturating_sub(1) {
        let (left, right) = decoded.split_at_mut(index + 1);
        if let (Decoded::Viofo(left), Decoded::Viofo(right)) = (&mut left[index], &mut right[0]) {
            viofo::validate_boundary(
                &requests[index].path,
                left,
                &requests[index + 1].path,
                right,
            );
        }
    }
    requests
        .iter()
        .zip(decoded)
        .map(|(req, decoded)| GpsBatchItem {
            file_path: req.path.clone(),
            points: match decoded {
                Decoded::Other(points) => points,
                Decoded::Viofo(points) => points.into_iter().map(|p| p.point).collect(),
            },
        })
        .collect()
}

/// Load archived trip-stitched GPS from the DB. Returns an empty vec
/// when no row exists for the trip — the frontend treats that as the
/// signal to fall back to the per-segment `extract_gps_batch` path
/// (which only succeeds when originals are still on disk).
#[tauri::command]
pub async fn load_trip_gps(
    trip_id: String,
    slot: State<'_, ArchiveSlot>,
) -> Result<Vec<GpsPoint>, AppError> {
    let db = require_db(&slot)?;
    let conn = db
        .lock()
        .map_err(|_| AppError::Internal("db mutex poisoned".into()))?;
    Ok(crate::db::trip_gps::load(&conn, &trip_id)?.unwrap_or_default())
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
        CameraKind::Mai70 => mai70::extract(path),
        CameraKind::Viofo => viofo::extract_video(path),
        // Generic fallback: try Wolf Box's decoder as a best-guess since
        // the ShenShu meta-track layout is the only one we know, but log
        // that we're guessing. Often this will just return an empty vec.
        CameraKind::Generic => shenshu::extract(path),
    }
}

#[cfg(test)]
mod batch_dispatch_tests {
    use super::*;

    #[test]
    fn non_viofo_points_and_request_order_pass_through_unchanged() {
        let requests = [
            GpsRequest {
                path: "rear.mp4".into(),
                camera_kind: CameraKind::Mai70,
            },
            GpsRequest {
                path: "front.mp4".into(),
                camera_kind: CameraKind::WolfBox,
            },
            GpsRequest {
                path: "generic.mp4".into(),
                camera_kind: CameraKind::Generic,
            },
        ];
        let points = |kind| {
            vec![GpsPoint {
                t_offset_s: 1.25,
                lat: if kind == CameraKind::Mai70 { 1.0 } else { 2.0 },
                lon: 3.0,
                speed_mps: 4.0,
                heading_deg: 5.0,
                altitude_m: 6.0,
                fix_ok: true,
            }]
        };
        let batch = extract_batch_with_other(&requests, |_, kind| points(kind));
        for (request, result) in requests.iter().zip(&batch) {
            assert_eq!(result.file_path, request.path);
            assert_eq!(
                serde_json::to_value(&result.points).unwrap(),
                serde_json::to_value(points(request.camera_kind)).unwrap()
            );
        }
    }
}
