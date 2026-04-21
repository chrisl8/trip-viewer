//! For each user-configured place, check whether any of the segment's
//! GPS points falls within the place's radius. Emits one tag per
//! matching place, named `place_<id>` and categorized as `place`. The
//! UI resolves the stable ID back to the place's display name so that
//! renaming a place doesn't orphan tags.
//!
//! Emits nothing when no places are configured, or when the segment
//! has no GPS support / no points.

use std::path::Path;

use crate::error::AppError;
use crate::gps::extract_for_kind;
use crate::scans::{CostTier, Scan, ScanContext};
use crate::tags::{Tag, TagCategory, TagSource};

pub struct GpsPlace;

/// Great-circle distance between two lat/lon pairs in meters.
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0; // Earth radius in meters
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let d_phi = (lat2 - lat1).to_radians();
    let d_lambda = (lon2 - lon1).to_radians();
    let a = (d_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (d_lambda / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    R * c
}

impl Scan for GpsPlace {
    fn id(&self) -> &'static str {
        "gps_place"
    }
    fn version(&self) -> u32 {
        1
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Medium
    }
    fn display_name(&self) -> &'static str {
        "Places"
    }
    fn description(&self) -> &'static str {
        "Tag segments that pass through any of your saved places (Home, Work, etc.). Does nothing if you haven't added any places yet. Re-run after adding, editing, or removing a place to update tags."
    }
    fn emits(&self) -> &'static [&'static str] {
        // Tag names are dynamic (`place_<id>`) so we can't enumerate
        // them statically. The UI resolves them via the places table.
        &["place_*"]
    }

    fn run(&self, ctx: &ScanContext) -> Result<Vec<Tag>, AppError> {
        if ctx.places.is_empty() || !ctx.segment.gps_supported {
            return Ok(vec![]);
        }
        let path = Path::new(&ctx.segment.master_path);
        let points = extract_for_kind(path, ctx.segment.camera_kind).unwrap_or_default();
        if points.is_empty() {
            return Ok(vec![]);
        }

        let mut tags = Vec::new();
        for place in ctx.places {
            let hit = points.iter().filter(|p| p.fix_ok).any(|p| {
                haversine_m(p.lat, p.lon, place.lat, place.lon) <= place.radius_m
            });
            if !hit {
                continue;
            }
            let tag_name = format!("place_{}", place.id);
            let metadata = serde_json::json!({
                "place_id": place.id,
                "place_name": place.name,
                "radius_m": place.radius_m,
            })
            .to_string();
            tags.push(Tag {
                id: None,
                segment_id: Some(ctx.segment.id.clone()),
                trip_id: None,
                name: tag_name,
                category: TagCategory::Place,
                source: TagSource::System,
                scan_id: Some(self.id().to_string()),
                scan_version: Some(self.version()),
                confidence: None,
                start_ms: None,
                end_ms: None,
                note: None,
                metadata_json: Some(metadata),
                created_ms: chrono::Utc::now().timestamp_millis(),
            });
        }
        Ok(tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_distances() {
        // Two points ~111km apart on the same meridian (1 degree of latitude).
        let d = haversine_m(37.0, -122.0, 38.0, -122.0);
        assert!((d - 111_195.0).abs() < 200.0, "got {d}m");

        // Same point → zero.
        let d0 = haversine_m(37.0, -122.0, 37.0, -122.0);
        assert!(d0 < 0.1);
    }

    #[test]
    fn haversine_within_hundred_meters() {
        // 0.001 degrees latitude ≈ 111m.
        let d = haversine_m(37.0, -122.0, 37.001, -122.0);
        assert!((d - 111.0).abs() < 5.0, "got {d}m");
    }
}
