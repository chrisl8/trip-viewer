import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import type { CameraKind } from "../types/model";

export type ExportLayout = "side" | "pip";
export type ExportSlotSize = "small" | "medium" | "large";
export type ExportPipEdge = "top" | "bottom";
export type ExportQuality = "high" | "balanced" | "small";
export type ExportCodec = "h264" | "h265";
export type ExportMapStyle = "route" | "streets";

export interface ExportSegmentArg {
  durationS: number;
  files: { label: string; path: string }[];
}

export interface ExportAudioArg {
  label: string;
  /** 1 = unchanged, 0.5 = half, 2 = double. */
  volume: number;
}

export interface StartExportArgs {
  outputPath: string;
  segments: ExportSegmentArg[];
  cameraKind: CameraKind;
  /** Seconds from the start of the trip. */
  startS: number;
  durationS: number;
  layout: ExportLayout;
  slotSize: ExportSlotSize;
  pipEdge: ExportPipEdge;
  quality: ExportQuality;
  codec: ExportCodec;
  mainLabel: string;
  /** Other channels to show small, in order. null = all of them. */
  slotLabels: string[] | null;
  showMap: boolean;
  mapStyle: ExportMapStyle;
  /** Empty = silent. */
  audio: ExportAudioArg[];
}

export interface ExportProgressEvent {
  fraction: number;
}

export interface ExportDoneEvent {
  ok: boolean;
  cancelled: boolean;
  outputPath: string | null;
  warnings: string[];
  error: string | null;
}

export function startExport(args: StartExportArgs): Promise<void> {
  return invoke<void>("start_export", { args });
}

export function cancelExport(): Promise<void> {
  return invoke<void>("cancel_export");
}

export function onExportProgress(
  cb: (e: ExportProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<ExportProgressEvent>("export:progress", (e) => cb(e.payload));
}

export function onExportDone(
  cb: (e: ExportDoneEvent) => void,
): Promise<UnlistenFn> {
  return listen<ExportDoneEvent>("export:done", (e) => cb(e.payload));
}

/** Ask where to save the clip. Resolves null if the user cancels. */
export async function pickExportPath(
  defaultName: string,
): Promise<string | null> {
  const selected = await save({
    title: "Export clip",
    defaultPath: defaultName,
    filters: [{ name: "MP4 video", extensions: ["mp4"] }],
  });
  return selected ?? null;
}
