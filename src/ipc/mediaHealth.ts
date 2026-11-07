import { invoke } from "@tauri-apps/api/core";

export interface MediaHealthRange {
  path: string;
  startS: number;
  endS: number | null;
  errorKind: string;
}

export function loadMediaHealth(paths: string[]): Promise<MediaHealthRange[]> {
  return invoke<MediaHealthRange[]>("load_media_health", { paths });
}

export function recordMediaFailure(
  path: string,
  startS: number,
  errorKind: string,
): Promise<void> {
  return invoke<void>("record_media_failure", { path, startS, errorKind });
}

export function recordMediaRecovery(
  path: string,
  endS: number,
): Promise<void> {
  return invoke<void>("record_media_recovery", { path, endS });
}
