//! EE filename-byte normalization. Reads `SegmentRecord.is_event` (already
//! decoded upstream from the Wolf Box `YYYY_MM_DD_HHMMSS_EE_C.MP4` format)
//! and emits a camera-sourced `event` tag when set. No file I/O.

use crate::error::AppError;
use crate::scans::{CostTier, Scan, ScanContext};
use crate::tags::{Tag, TagCategory, TagSource};

pub struct EeNormalize;

impl Scan for EeNormalize {
    fn id(&self) -> &'static str {
        "ee_normalize"
    }
    fn version(&self) -> u32 {
        1
    }
    fn cost_tier(&self) -> CostTier {
        CostTier::Cheap
    }
    fn display_name(&self) -> &'static str {
        "Camera events"
    }
    fn description(&self) -> &'static str {
        "Tag segments the dashcam itself marked as events (e.g. G-sensor triggers, manual event button). Reads a flag the camera already wrote into the filename — instant, no file reads."
    }
    fn emits(&self) -> &'static [&'static str] {
        &["event"]
    }

    fn run(&self, ctx: &ScanContext) -> Result<Vec<Tag>, AppError> {
        if !ctx.segment.is_event {
            return Ok(vec![]);
        }
        // Event-flagged segments carry a camera-sourced tag so it shows
        // the same yellow/event color as other camera events, and so
        // re-running scans doesn't clobber user tags on this segment.
        Ok(vec![Tag {
            id: None,
            segment_id: Some(ctx.segment.id.clone()),
            trip_id: None,
            name: "event".to_string(),
            category: TagCategory::Event,
            source: TagSource::Camera,
            scan_id: Some(self.id().to_string()),
            scan_version: Some(self.version()),
            confidence: None,
            start_ms: None,
            end_ms: None,
            note: None,
            metadata_json: None,
            created_ms: chrono::Utc::now().timestamp_millis(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::segments::SegmentRecord;
    use crate::scan::naming::CameraKind;

    fn fake_segment(is_event: bool) -> SegmentRecord {
        SegmentRecord {
            id: "seg-a".into(),
            trip_id: "trip-a".into(),
            master_path: "/fake".into(),
            is_event,
            camera_kind: CameraKind::WolfBox,
            gps_supported: true,
            duration_s: 60.0,
            is_tombstone: false,
        }
    }

    #[test]
    fn emits_event_tag_when_is_event() {
        let seg = fake_segment(true);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = ScanContext {
            segment: &seg,
            cancel: &cancel,
            places: &[],
        };
        let tags = EeNormalize.run(&ctx).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "event");
        assert_eq!(tags[0].category, TagCategory::Event);
        assert_eq!(tags[0].source, TagSource::Camera);
    }

    #[test]
    fn emits_nothing_when_not_event() {
        let seg = fake_segment(false);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = ScanContext {
            segment: &seg,
            cancel: &cancel,
            places: &[],
        };
        let tags = EeNormalize.run(&ctx).unwrap();
        assert!(tags.is_empty());
    }
}
