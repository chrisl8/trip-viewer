import { invoke } from "@tauri-apps/api/core";
import type { ChannelMeta, ScanResult } from "../types/model";

export function scanFolder(path: string): Promise<ScanResult> {
  return invoke<ScanResult>("scan_folder", { path });
}

export function probeFile(path: string): Promise<ChannelMeta> {
  return invoke<ChannelMeta>("probe_file", { path });
}
