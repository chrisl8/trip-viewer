#![allow(dead_code)] // wired up incrementally through upcoming tasks

use serde::{Deserialize, Serialize};

pub mod commands;
pub mod vocabulary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagCategory {
    Event,
    Motion,
    Audio,
    Quality,
    User,
    Place,
}

impl TagCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Motion => "motion",
            Self::Audio => "audio",
            Self::Quality => "quality",
            Self::User => "user",
            Self::Place => "place",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "event" => Some(Self::Event),
            "motion" => Some(Self::Motion),
            "audio" => Some(Self::Audio),
            "quality" => Some(Self::Quality),
            "user" => Some(Self::User),
            "place" => Some(Self::Place),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagSource {
    System,
    Camera,
    User,
}

impl TagSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Camera => "camera",
            Self::User => "user",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "camera" => Some(Self::Camera),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// A tag attached to a segment or trip. Exactly one of `segment_id` /
/// `trip_id` is populated; the DB enforces this via a CHECK constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// DB row id; `None` when the tag has not yet been inserted.
    pub id: Option<i64>,
    pub segment_id: Option<String>,
    pub trip_id: Option<String>,
    pub name: String,
    pub category: TagCategory,
    pub source: TagSource,
    pub scan_id: Option<String>,
    pub scan_version: Option<u32>,
    pub confidence: Option<f32>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub note: Option<String>,
    pub metadata_json: Option<String>,
    pub created_ms: i64,
}

impl Tag {
    pub fn new_segment_system(segment_id: String, name: &str, category: TagCategory, scan_id: &str, scan_version: u32) -> Self {
        Self {
            id: None,
            segment_id: Some(segment_id),
            trip_id: None,
            name: name.to_string(),
            category,
            source: TagSource::System,
            scan_id: Some(scan_id.to_string()),
            scan_version: Some(scan_version),
            confidence: None,
            start_ms: None,
            end_ms: None,
            note: None,
            metadata_json: None,
            created_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    pub fn new_segment_camera(segment_id: String, name: &str, category: TagCategory) -> Self {
        Self {
            id: None,
            segment_id: Some(segment_id),
            trip_id: None,
            name: name.to_string(),
            category,
            source: TagSource::Camera,
            scan_id: None,
            scan_version: None,
            confidence: None,
            start_ms: None,
            end_ms: None,
            note: None,
            metadata_json: None,
            created_ms: chrono::Utc::now().timestamp_millis(),
        }
    }
}
