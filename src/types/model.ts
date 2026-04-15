/** Canonical built-in labels. `Channel.label` is free-form (any string). */
export const LABEL_FRONT = "Front";
export const LABEL_INTERIOR = "Interior";
export const LABEL_REAR = "Rear";

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

export interface ScanError {
  path: string;
  reason: string;
}

export interface ScanResult {
  trips: Trip[];
  unmatched: string[];
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
