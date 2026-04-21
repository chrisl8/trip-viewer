//! Flags a segment as `stationary` if GPS speed stays below a threshold
//! for the entire recording. Uses the per-point `speed_mps` the Wolf Box
//! GPS decoder already extracts; does not fall back to lat/lon distance
//! since that would re-invent speed estimation.
//!
//! Threshold trade-off: 0.5 m/s (~1.8 km/h) tolerates GPS jitter while a
//! vehicle is truly idle. Raise if too many driving segments misclassify,
//! lower if idling-with-engine-on ends up tagged.

use std::path::Path;

use crate::error::AppError;
use crate::gps::extract_for_kind;
use crate::scans::{CostTier, Scan, ScanContext};
use crate::tags::{Tag, TagCategory, TagSource};

pub struct GpsStationary {
    pub threshold_mps: f64,
    pub min_points: usize,
}

impl Default for GpsStationary {
    fn default() -> Self {
        Self {
            threshold_mps: 0.5,
            // Need at least this many GPS points to trust the verdict;
            // a clip with 2 points and both stationary might just be
            // poor fix quality, not actually idle.
            min_points: 10,
        }
    }
}

impl Scan for GpsStationary {
    fn id(&self) -> &'static str {
        "gps_stationary"
    }
    fn version(&self) -> u32 {
        1
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Medium
    }
    fn display_name(&self) -> &'static str {
        "GPS stationary"
    }
    fn description(&self) -> &'static str {
        "Tag segments whose GPS speed stays near zero for the whole recording — typical of parked-in-garage footage. Skipped for cameras that don't record GPS."
    }
    fn emits(&self) -> &'static [&'static str] {
        &["stationary"]
    }

    fn run(&self, ctx: &ScanContext) -> Result<Vec<Tag>, AppError> {
        if !ctx.segment.gps_supported {
            return Ok(vec![]);
        }
        let path = Path::new(&ctx.segment.master_path);
        let points = extract_for_kind(path, ctx.segment.camera_kind).unwrap_or_default();
        if points.len() < self.min_points {
            return Ok(vec![]);
        }
        // Require every GPS-locked point to be below the threshold.
        // Points without a fix are ignored rather than counted as
        // stationary — an unlocked GPS isn't evidence of motion either way.
        let has_motion = points
            .iter()
            .filter(|p| p.fix_ok)
            .any(|p| p.speed_mps > self.threshold_mps);
        if has_motion {
            return Ok(vec![]);
        }
        let locked = points.iter().filter(|p| p.fix_ok).count();
        if locked < self.min_points {
            return Ok(vec![]);
        }

        let metadata = serde_json::json!({
            "locked_points": locked,
            "total_points": points.len(),
            "threshold_mps": self.threshold_mps,
        })
        .to_string();

        Ok(vec![Tag {
            id: None,
            segment_id: Some(ctx.segment.id.clone()),
            trip_id: None,
            name: "stationary".to_string(),
            category: TagCategory::Motion,
            source: TagSource::System,
            scan_id: Some(self.id().to_string()),
            scan_version: Some(self.version()),
            confidence: Some(1.0),
            start_ms: None,
            end_ms: None,
            note: None,
            metadata_json: Some(metadata),
            created_ms: chrono::Utc::now().timestamp_millis(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use crate::model::GpsPoint;
    use crate::scans::gps_stationary::GpsStationary;

    fn make_point(speed: f64, fix: bool) -> GpsPoint {
        GpsPoint {
            t_offset_s: 0.0,
            lat: 0.0,
            lon: 0.0,
            speed_mps: speed,
            heading_deg: 0.0,
            altitude_m: 0.0,
            fix_ok: fix,
        }
    }

    // Verify the core predicate (separated from the file-reading path
    // so we don't need fixture MP4s here).
    fn classify(points: &[GpsPoint], cfg: &GpsStationary) -> bool {
        if points.len() < cfg.min_points {
            return false;
        }
        let has_motion = points
            .iter()
            .filter(|p| p.fix_ok)
            .any(|p| p.speed_mps > cfg.threshold_mps);
        if has_motion {
            return false;
        }
        let locked = points.iter().filter(|p| p.fix_ok).count();
        locked >= cfg.min_points
    }

    #[test]
    fn all_slow_and_locked_classifies_stationary() {
        let cfg = GpsStationary::default();
        let points: Vec<_> = (0..20).map(|_| make_point(0.1, true)).collect();
        assert!(classify(&points, &cfg));
    }

    #[test]
    fn any_fast_point_disqualifies() {
        let cfg = GpsStationary::default();
        let mut points: Vec<_> = (0..20).map(|_| make_point(0.1, true)).collect();
        points[5] = make_point(5.0, true);
        assert!(!classify(&points, &cfg));
    }

    #[test]
    fn too_few_locked_points_returns_false() {
        let cfg = GpsStationary::default();
        let points: Vec<_> = (0..5).map(|_| make_point(0.1, true)).collect();
        assert!(!classify(&points, &cfg));
    }

    #[test]
    fn unlocked_points_are_ignored_for_motion_check() {
        let cfg = GpsStationary::default();
        let mut points: Vec<_> = (0..20).map(|_| make_point(0.1, true)).collect();
        points[3] = make_point(50.0, false); // unlocked, should not count as motion
        assert!(classify(&points, &cfg));
    }
}
