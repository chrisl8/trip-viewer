import { invoke } from "@tauri-apps/api/core";
import type { CameraKind, GpsBatchItem, GpsPoint } from "../types/model";

/**
 * A GPS extraction request. The backend dispatches to a brand-specific
 * decoder based on `cameraKind` because each dashcam stores GPS in its own
 * proprietary layout (ShenShu meta-track for Wolf Box, gps0 atom for
 * Miltona, nothing at all for Thinkware, etc.). Pairs each segment's
 * master channel path with the `cameraKind` the scanner identified.
 */
export interface GpsRequest {
  path: string;
  cameraKind: CameraKind;
}

export function extractGps(
  path: string,
  cameraKind: CameraKind,
): Promise<GpsPoint[]> {
  return invoke<GpsPoint[]>("extract_gps", { path, cameraKind });
}

export function extractGpsBatch(
  requests: GpsRequest[],
): Promise<GpsBatchItem[]> {
  return invoke<GpsBatchItem[]>("extract_gps_batch", { requests });
}

/**
 * Write a diagnostic dump of a Miltona `.MOV` file's `gps0` atom. Used by
 * the "Export GPS debug" UI button to collect ground-truth samples while
 * the lat/lon encoding is still being finalized. Returns the path of the
 * written text file so the UI can show it / offer to open it.
 */
export function dumpMiltonaGpsDebug(path: string): Promise<string> {
  return invoke<string>("dump_miltona_gps_debug", { path });
}
