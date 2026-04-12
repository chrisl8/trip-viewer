use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Front,
    Interior,
    Rear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub kind: ChannelKind,
    pub file_path: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps_num: Option<u32>,
    pub fps_den: Option<u32>,
    pub codec: Option<String>,
    pub has_gpmd_track: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: Uuid,
    pub start_time: NaiveDateTime,
    pub duration_s: f64,
    pub is_event: bool,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trip {
    pub id: Uuid,
    pub start_time: NaiveDateTime,
    pub end_time: NaiveDateTime,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsPoint {
    pub t_offset_s: f64,
    pub lat: f64,
    pub lon: f64,
    pub speed_mps: f64,
    pub heading_deg: f64,
    pub altitude_m: f64,
    pub fix_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsBatchItem {
    pub file_path: String,
    pub points: Vec<GpsPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub trips: Vec<Trip>,
    pub unmatched: Vec<String>,
    pub errors: Vec<ScanError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMeta {
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub codec: String,
    pub has_gpmd_track: bool,
}
