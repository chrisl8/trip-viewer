import { invoke } from "@tauri-apps/api/core";
import type { GpsBatchItem, GpsPoint } from "../types/model";

export function extractGps(path: string): Promise<GpsPoint[]> {
  return invoke<GpsPoint[]>("extract_gps", { path });
}

export function extractGpsBatch(paths: string[]): Promise<GpsBatchItem[]> {
  return invoke<GpsBatchItem[]>("extract_gps_batch", { paths });
}
