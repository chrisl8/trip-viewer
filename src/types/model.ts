/** Canonical built-in labels. `Channel.label` is free-form (any string). */
export const LABEL_FRONT = "Front";
export const LABEL_INTERIOR = "Interior";
export const LABEL_REAR = "Rear";

/**
 * Which dashcam produced a file/segment. Serialized as lowercase camelCase
 * to match the Rust `#[serde(rename_all = "camelCase")]` on `CameraKind`.
 */
export type CameraKind = "wolfBox" | "thinkware" | "miltona" | "generic";

export interface Channel {
  /**
   * Free-form, user-visible label ("Front", "Interior", "Rear",
   * "Channel A", etc.). Produced by the Rust filename parser.
   */
  label: string;
  filePath: string;
  width: number | null;
  height: number | null;
  fpsNum: number | null;
  fpsDen: number | null;
  codec: string | null;
  hasGpmdTrack: boolean;
}

export interface Segment {
  id: string;
  startTime: string;
  durationS: number;
  isEvent: boolean;
  /** Channels in canonical order. channels[0] is the sync master. */
  channels: Channel[];
  /** Which dashcam brand recorded this segment (derived from filename). */
  cameraKind: CameraKind;
  /**
   * Whether the frontend should render the GPS map for this segment.
   * False for camera models we know don't record GPS (e.g. Thinkware
   * non-GPS variants). When false, the map panel is hidden entirely and
   * a small inline caption explains why — rather than showing an empty
   * "No GPS data" placeholder that eats screen real estate.
   */
  gpsSupported: boolean;
  /**
   * Sum of the on-disk size of every channel file in the segment.
   * `null` when stat failed at scan time, or for segments persisted
   * before migration 0009 that haven't been re-scanned yet.
   */
  sizeBytes: number | null;
  /**
   * Tags attached to this segment. Present when the caller has loaded
   * tags into the trip — live-updating state lives in the tagsSlice,
   * so renderers that need to react to scan/user-tag changes should
   * read from the slice keyed by segment.id rather than this field.
   */
  tags?: Tag[];
}

export interface Trip {
  id: string;
  startTime: string;
  endTime: string;
  segments: Segment[];
  /**
   * Mirrors `Segment.cameraKind`. Persisted on the trip row so a trip
   * with no segments left on disk (archive-only — only the timelapse
   * remains) still has the metadata playback needs.
   */
  cameraKind: CameraKind;
  /** Mirrors `Segment.gpsSupported`, same rationale as `cameraKind`. */
  gpsSupported: boolean;
  /**
   * True when the trip's source segments have all been deleted but its
   * timelapse pre-render(s) remain. The trip is still discoverable in
   * the sidebar; only tier playback is available — Original is hidden.
   */
  archiveOnly?: boolean;
  /** Trip-level tags. Same caveat as `Segment.tags` re: slice vs. field. */
  tags?: Tag[];
}

export interface GpsPoint {
  tOffsetS: number;
  lat: number;
  lon: number;
  speedMps: number;
  headingDeg: number;
  altitudeM: number;
  fixOk: boolean;
}

export interface GpsBatchItem {
  filePath: string;
  points: GpsPoint[];
}

/**
 * Category of scan failure. Mirrors the Rust `ScanErrorKind` enum with
 * camelCase serde renaming applied.
 */
export type ScanErrorKind =
  | "invalidFilename"
  | "fileUnreadable"
  | "mp4MoovMissing"
  | "mp4BoxOverflow"
  | "mp4NoVideoTrack"
  | "mp4Other";

export interface ScanError {
  path: string;
  kind: ScanErrorKind;
  /** Short, human-readable one-liner for the Reason column. */
  message: string;
  /** Raw technical detail, if any. Not displayed in v1; kept for a future
   *  row-expand UI so the data shape doesn't have to change twice. */
  detail: string | null;
  /** File size in bytes if fs::metadata succeeded on the scan side. */
  sizeBytes: number | null;
  /** Last-modified time as Unix epoch milliseconds. */
  modifiedMs: number | null;
}

export interface ScanResult {
  trips: Trip[];
  errors: ScanError[];
}

export interface ChannelMeta {
  durationS: number;
  width: number;
  height: number;
  fpsNum: number;
  fpsDen: number;
  codec: string;
  hasGpmdTrack: boolean;
}

/**
 * Category a tag belongs to. Drives color mapping in the timeline,
 * sidebar badges, and tag pills. Mirrors the Rust `TagCategory` enum.
 */
export type TagCategory =
  | "event"
  | "motion"
  | "audio"
  | "quality"
  | "user"
  | "place";

/**
 * Where a tag came from. `system` tags are emitted by scans and get
 * replaced when the scan re-runs. `camera` tags come from firmware-level
 * metadata (e.g. Wolf Box EE flag). `user` tags are applied manually
 * and are never touched by scans.
 */
export type TagSource = "system" | "camera" | "user";

export interface Tag {
  id: number | null;
  segmentId: string | null;
  tripId: string | null;
  name: string;
  category: TagCategory;
  source: TagSource;
  scanId: string | null;
  scanVersion: number | null;
  confidence: number | null;
  startMs: number | null;
  endMs: number | null;
  note: string | null;
  metadataJson: string | null;
  createdMs: number;
}

/**
 * Progress event payload emitted by the Rust scan worker. Batched every
 * ~250ms to avoid flooding IPC.
 */
export interface ScanProgress {
  total: number;
  done: number;
  failed: number;
  currentSegmentId: string | null;
  currentScanId: string | null;
}
