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
}

export interface Trip {
  id: string;
  startTime: string;
  endTime: string;
  segments: Segment[];
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
