use crate::scan::naming::CameraKind;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fixed UUID namespace for deriving deterministic segment and trip IDs
/// via UUIDv5. Changing this value invalidates every previously-stored
/// tag in the DB, so treat it as load-bearing.
const TRIPVIEWER_ID_NS: Uuid = Uuid::from_bytes([
    0x5d, 0x11, 0x77, 0x2f, 0x8e, 0x3a, 0x4c, 0x22,
    0xa1, 0x9d, 0xf5, 0x6c, 0x3b, 0x8d, 0x7a, 0x4e,
]);

/// Derive a stable segment ID from its master file path plus start time.
/// The same (path, start) always yields the same UUID, so tags persisted
/// in the DB re-attach on folder rescans as long as neither changed.
pub fn derive_segment_id(master_path: &str, start_time: NaiveDateTime) -> Uuid {
    let key = format!("seg|{}|{}", master_path, start_time.and_utc().timestamp_millis());
    Uuid::new_v5(&TRIPVIEWER_ID_NS, key.as_bytes())
}

/// Derive a trip ID from its first segment's ID so trip-level user tags
/// survive folder rescans.
pub fn derive_trip_id(first_segment_id: Uuid) -> Uuid {
    Uuid::new_v5(&TRIPVIEWER_ID_NS, first_segment_id.as_bytes())
}

/// Canonical channel labels for common cases. Used for ordering and for
/// the built-in parsers, but `Channel.label` is free-form so new cameras
/// can introduce arbitrary labels without a schema change.
pub const LABEL_FRONT: &str = "Front";
pub const LABEL_INTERIOR: &str = "Interior";
pub const LABEL_REAR: &str = "Rear";

/// Canonical sort rank for a channel label. Lower = earlier in the list.
/// Known Wolf Box / Thinkware layouts sort first; anything else goes after
/// in alphabetical order. This gives us a stable master channel choice
/// (always channels[0]) and a stable UI ordering across any camera.
pub fn label_rank(label: &str) -> (u8, String) {
    let primary: u8 = match label {
        LABEL_FRONT => 0,
        LABEL_INTERIOR => 1,
        LABEL_REAR => 2,
        _ => 10,
    };
    (primary, label.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    /// Free-form, user-visible label ("Front", "Interior", "Rear",
    /// "Channel A", etc.). Produced by the filename parser.
    pub label: String,
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
    /// Channels in canonical order (see `label_rank`). The first entry is
    /// the sync master.
    pub channels: Vec<Channel>,
    /// Which dashcam brand recorded this segment. Derived from the master
    /// channel's filename by the scanner.
    pub camera_kind: CameraKind,
    /// Whether the frontend should render the GPS map for this segment.
    /// `false` when we know this camera model doesn't record GPS (e.g.
    /// Thinkware non-GPS variants) — the UI hides the panel entirely and
    /// shows a small caption instead of an empty "No GPS data" placeholder.
    pub gps_supported: bool,
    /// Sum of the on-disk size of every channel file in the segment.
    /// `None` when stat'ing failed (file vanished mid-scan, permissions)
    /// or when the row was persisted before migration 0009 and hasn't
    /// been touched by a scan since. Frontend renders `None` as "—".
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// True when the user deleted this segment's originals but the trip
    /// has a timelapse archive that covers its time range. The row is
    /// kept so the timeline can render a hatched gap and the player can
    /// auto-switch to a tier across the deleted span. `channels` is `[]`
    /// for tombstones; `master_path` is `''` in the DB.
    #[serde(default)]
    pub is_tombstone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trip {
    pub id: Uuid,
    pub start_time: NaiveDateTime,
    pub end_time: NaiveDateTime,
    pub segments: Vec<Segment>,
    /// Mirrors `Segment.camera_kind` so a trip whose source segments
    /// have been deleted (archive-only — only the timelapse remains)
    /// can still be played back without needing to read from a
    /// non-existent segment.
    pub camera_kind: CameraKind,
    /// Mirrors `Segment.gps_supported`, same rationale as `camera_kind`.
    pub gps_supported: bool,
    /// True when this trip has no source segments left on disk and is
    /// only viewable via its timelapse pre-render(s). Set by the
    /// archive-only loader; always false for trips returned by
    /// `scan_folder`.
    #[serde(default)]
    pub archive_only: bool,
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

/// Category of scan failure. Each kind comes with a short user-facing
/// message produced by `scan::errors::classify`; the UI renders this as
/// a colored pill so the user can tell at a glance whether the file is
/// repairable (e.g. missing moov) vs structurally corrupt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScanErrorKind {
    /// No filename parser matched. Usually a stray non-dashcam file.
    InvalidFilename,
    /// Open/read failed — permissions, drive disconnected mid-scan, etc.
    FileUnreadable,
    /// MP4 parsed but has no `moov` atom. File was not closed properly;
    /// underlying media bytes are usually intact and can often be recovered
    /// with external tools given a reference file from the same camera.
    Mp4MoovMissing,
    /// A box header declares more bytes than the file contains. Truncated
    /// mid-box-write. Recovery difficulty depends on which box was hit.
    Mp4BoxOverflow,
    /// MP4 structurally valid but no video track found.
    Mp4NoVideoTrack,
    /// Any other mp4-crate parse failure.
    Mp4Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub path: String,
    pub kind: ScanErrorKind,
    /// Short, human-readable one-liner for the Reason column.
    pub message: String,
    /// Raw technical detail (original mp4-crate / IO error text) preserved
    /// for a future row-expand UI. None when the short message already says
    /// everything useful.
    pub detail: Option<String>,
    /// File size in bytes if `fs::metadata` succeeded. None if the file
    /// disappeared between walk and probe or metadata access was denied.
    pub size_bytes: Option<u64>,
    /// Last-modified time as Unix epoch milliseconds.
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub trips: Vec<Trip>,
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
