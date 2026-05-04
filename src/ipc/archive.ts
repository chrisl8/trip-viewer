import { invoke } from "@tauri-apps/api/core";

export interface CurrentArchive {
  root: string;
  label: string;
}

export interface RecentArchive {
  path: string;
  label: string;
  lastOpenedMs: number;
  online: boolean;
}

/**
 * Open the archive at `path`, replacing any currently-open archive.
 * Persists `last_archive` and bumps the recent list. Returns the
 * resolved info (canonicalized path + display label).
 */
export function openArchive(path: string): Promise<CurrentArchive> {
  return invoke<CurrentArchive>("open_archive", { path });
}

export function closeArchive(): Promise<void> {
  return invoke<void>("close_archive");
}

export function currentArchive(): Promise<CurrentArchive | null> {
  return invoke<CurrentArchive | null>("current_archive");
}

export function listRecentArchives(): Promise<RecentArchive[]> {
  return invoke<RecentArchive[]>("list_recent_archives");
}

export function forgetArchive(path: string): Promise<void> {
  return invoke<void>("forget_archive", { path });
}
