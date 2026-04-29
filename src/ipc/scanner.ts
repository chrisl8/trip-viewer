import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ChannelMeta, ScanProgress, ScanResult } from "../types/model";

export function scanFolder(path: string): Promise<ScanResult> {
  return invoke<ScanResult>("scan_folder", { path });
}

export function probeFile(path: string): Promise<ChannelMeta> {
  return invoke<ChannelMeta>("probe_file", { path });
}

// ---- Analysis scan pipeline (tag-producing scans) ---------------------

export type ScanScope = "newOnly" | "rescanStale" | "all";

export type CostTier = "cheap" | "medium" | "heavy";

export interface ScanDescriptor {
  id: string;
  displayName: string;
  description: string;
  version: number;
  costTier: CostTier;
  emits: string[];
}

export interface ScanStartEvent {
  total: number;
  scanIds: string[];
}

export interface ScanDoneEvent {
  total: number;
  done: number;
  failed: number;
  tagsEmitted: number;
  cancelled: boolean;
}

export interface ScanCoverage {
  scanId: string;
  totalSegments: number;
  doneCount: number;
  staleCount: number;
  failedCount: number;
  notRunCount: number;
  /** Up to 3 distinct error_message strings from failed runs for
   *  this (trip, scan). Empty when failedCount === 0. */
  sampleFailures: string[];
}

export interface TripScanCoverage {
  tripId: string;
  perScan: ScanCoverage[];
}

export function listScans(): Promise<ScanDescriptor[]> {
  return invoke<ScanDescriptor[]>("list_scans");
}

export function listScanCoverage(): Promise<TripScanCoverage[]> {
  return invoke<TripScanCoverage[]>("list_scan_coverage");
}

export function startAnalysisScan(
  scanIds: string[],
  scope: ScanScope,
  tripIds?: string[] | null,
): Promise<void> {
  return invoke<void>("start_scan", {
    scanIds,
    scope,
    tripIds: tripIds ?? null,
  });
}

export function cancelAnalysisScan(): Promise<void> {
  return invoke<void>("cancel_scan");
}

export function onScanStart(
  cb: (e: ScanStartEvent) => void,
): Promise<UnlistenFn> {
  return listen<ScanStartEvent>("scan:start", (e) => cb(e.payload));
}

export function onScanProgress(
  cb: (e: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan:progress", (e) => cb(e.payload));
}

export function onScanDone(
  cb: (e: ScanDoneEvent) => void,
): Promise<UnlistenFn> {
  return listen<ScanDoneEvent>("scan:done", (e) => cb(e.payload));
}
