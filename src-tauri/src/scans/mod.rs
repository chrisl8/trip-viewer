//! Video-analysis pipeline. Each `Scan` examines one segment and emits
//! zero or more neutral, descriptive tags that get persisted to the DB
//! and surfaced in the UI (timeline bands, sidebar counts, review view).
//!
//! Build-order note: this task (5) ships a synchronous runner only. Task
//! 7 refactors `start_scan` into a tokio supervisor with cost-tier
//! semaphores, progress events, and cancellation; the `Scan` trait stays
//! stable across that refactor.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::db::places::Place;
use crate::db::segments::SegmentRecord;
use crate::error::AppError;
use crate::tags::Tag;

pub mod audio_rms;
pub mod commands;
pub mod ee_normalize;
pub mod gps_place;
pub mod gps_stationary;
pub mod worker;

/// Shared cancel flag visible to both the worker loop and individual
/// scans. Heavy scans should poll this in their inner loops so cancel
/// is felt sub-second even mid-decode.
pub type CancelFlag = Arc<AtomicBool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostTier {
    Cheap,
    Medium,
    Heavy,
}

pub struct ScanContext<'a> {
    pub segment: &'a SegmentRecord,
    pub cancel: &'a CancelFlag,
    /// Places loaded once at the start of the scan run. Empty when no
    /// places are configured. Scans that don't care about places (most
    /// of them) simply ignore this field.
    pub places: &'a [Place],
}

pub trait Scan: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> u32;
    fn cost_tier(&self) -> CostTier;
    /// Short user-facing label shown in the Scan launcher. Use title
    /// case, no trailing punctuation.
    fn display_name(&self) -> &'static str;
    /// One-sentence explanation of what this scan looks for and what
    /// tags it emits. Shown below the display name in the launcher.
    fn description(&self) -> &'static str;
    /// Tag names this scan can emit. Used by the UI to show per-scan
    /// checkboxes in the Scan launcher.
    fn emits(&self) -> &'static [&'static str];
    fn run(&self, ctx: &ScanContext) -> Result<Vec<Tag>, AppError>;
}

pub fn registry() -> Vec<Box<dyn Scan>> {
    vec![
        Box::new(ee_normalize::EeNormalize),
        Box::new(gps_stationary::GpsStationary::default()),
        Box::new(audio_rms::AudioRms::default()),
        Box::new(gps_place::GpsPlace),
    ]
}

pub fn find_scan(scan_id: &str) -> Option<Box<dyn Scan>> {
    registry().into_iter().find(|s| s.id() == scan_id)
}
