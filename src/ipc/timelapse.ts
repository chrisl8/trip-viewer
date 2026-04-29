import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export type TimelapseTier = "8x" | "16x" | "60x";
export type TimelapseChannel = "F" | "I" | "R";
export type TimelapseJobStatus = "pending" | "running" | "done" | "failed";
export type TimelapseJobScope = "newOnly" | "failedOnly" | "rebuildAll";

export interface FfmpegCapabilities {
  version: string;
  nvencHevc: boolean;
}

export interface TimelapseSettings {
  ffmpegPath: string | null;
  capabilities: FfmpegCapabilities | null;
}

export interface TimelapseJobRow {
  tripId: string;
  tier: string;
  channel: string;
  status: TimelapseJobStatus;
  outputPath: string | null;
  errorMessage: string | null;
  ffmpegVersion: string | null;
  encoderUsed: string | null;
  createdAtMs: number;
  completedAtMs: number | null;
  /** Number of source segments that were filled with black-frame
   *  placeholders because the real sibling file was missing. Zero
   *  for clean runs. Non-zero means the output is valid and in sync
   *  but shows black on this channel for those segments. */
  paddedCount: number;
  /** JSON-serialized `CurveSegment[]` produced at encode time. Null
   *  for legacy rows (pre-speed-curve column) and pending/failed
   *  rows. The player uses this to map file-time ↔ concat-time in
   *  tiered playback. */
  speedCurveJson: string | null;
  /** On-disk size of `outputPath` in bytes. Null for non-done rows
   *  and for done rows encoded before migration 0009 whose backfill
   *  hasn't filled the column yet (or whose output file is missing). */
  outputSizeBytes: number | null;
}

export interface TimelapseStartEvent {
  total: number;
  tiers: string[];
}

export interface TimelapseProgressEvent {
  total: number;
  done: number;
  failed: number;
  currentTripId: string | null;
  currentTier: string | null;
  currentChannel: string | null;
}

export interface TimelapseDoneEvent {
  total: number;
  done: number;
  failed: number;
  cancelled: boolean;
}

export interface StartTimelapseArgs {
  tripIds: string[] | null;
  tiers: TimelapseTier[];
  channels: TimelapseChannel[];
  scope: TimelapseJobScope;
}

// ── Commands ───────────────────────────────────────────────────────────

export function getTimelapseSettings(): Promise<TimelapseSettings> {
  return invoke<TimelapseSettings>("get_timelapse_settings");
}

export function testFfmpeg(path: string): Promise<FfmpegCapabilities> {
  return invoke<FfmpegCapabilities>("test_ffmpeg", { path });
}

/** Wipe the cached ffmpeg path + capabilities from settings. After
 *  this resolves, `getTimelapseSettings()` returns nulls. */
export function clearTimelapseSettings(): Promise<void> {
  return invoke<void>("clear_timelapse_settings");
}

export function startTimelapse(args: StartTimelapseArgs): Promise<void> {
  return invoke<void>("start_timelapse", { args });
}

export function cancelTimelapse(): Promise<void> {
  return invoke<void>("cancel_timelapse");
}

export function listTimelapseJobs(): Promise<TimelapseJobRow[]> {
  return invoke<TimelapseJobRow[]>("list_timelapse_jobs");
}

/**
 * File-picker for the ffmpeg binary. Filters to .exe on Windows; Unix
 * systems allow any file since the binary has no extension. Returns
 * the selected absolute path or null if the user cancelled.
 */
export async function pickFfmpegBinary(): Promise<string | null> {
  const isWindows = navigator.userAgent.toLowerCase().includes("windows");
  const selected = await open({
    multiple: false,
    directory: false,
    title: "Locate your ffmpeg executable",
    filters: isWindows
      ? [{ name: "Executable", extensions: ["exe"] }]
      : undefined,
  });
  if (typeof selected === "string") return selected;
  return null;
}

// ── Event listeners ────────────────────────────────────────────────────

export function onTimelapseStart(
  cb: (e: TimelapseStartEvent) => void,
): Promise<UnlistenFn> {
  return listen<TimelapseStartEvent>("timelapse:start", (e) => cb(e.payload));
}

export function onTimelapseProgress(
  cb: (e: TimelapseProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<TimelapseProgressEvent>("timelapse:progress", (e) =>
    cb(e.payload),
  );
}

export function onTimelapseDone(
  cb: (e: TimelapseDoneEvent) => void,
): Promise<UnlistenFn> {
  return listen<TimelapseDoneEvent>("timelapse:done", (e) => cb(e.payload));
}
