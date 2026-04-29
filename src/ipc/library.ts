import { invoke } from "@tauri-apps/api/core";

export interface LibraryStorageSummary {
  totalBytes: number;
  originalsBytes: number;
  timelapseBytes: number;
  reclaimableBytes: number;
  reclaimableTripIds: string[];
}

export function getLibraryStorageSummary(): Promise<LibraryStorageSummary> {
  return invoke<LibraryStorageSummary>("get_library_storage_summary");
}
