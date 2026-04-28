import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ImportSource,
  ImportProgress,
  ImportPhaseChange,
  ImportWarning,
  UnknownFile,
  UnknownFileDecision,
  ImportResult,
} from "../types/import";

export function discoverSources(): Promise<ImportSource[]> {
  return invoke<ImportSource[]>("discover_sources");
}

export function startImport(
  rootPath: string,
  sources: ImportSource[],
): Promise<void> {
  return invoke("start_import", { rootPath, sources });
}

/**
 * Non-destructive variant of startImport: copy MP4/MOV files from an
 * arbitrary folder into the library, leaving the source folder intact.
 * Same hash-while-copy + verified-destination guarantees as the SD-card
 * flow; emits the same `import:*` events.
 */
export function startFolderImport(
  rootPath: string,
  sourcePath: string,
): Promise<void> {
  return invoke("start_folder_import", { rootPath, sourcePath });
}

export function cancelImport(): Promise<void> {
  return invoke("cancel_import");
}

export function resolveUnknowns(
  decisions: UnknownFileDecision[],
): Promise<void> {
  return invoke("resolve_unknowns", { decisions });
}

export function onImportPhase(
  cb: (e: ImportPhaseChange) => void,
): Promise<UnlistenFn> {
  return listen<ImportPhaseChange>("import:phase", (e) => cb(e.payload));
}

export function onImportProgress(
  cb: (e: ImportProgress) => void,
): Promise<UnlistenFn> {
  return listen<ImportProgress>("import:progress", (e) => cb(e.payload));
}

export function onImportWarning(
  cb: (e: ImportWarning) => void,
): Promise<UnlistenFn> {
  return listen<ImportWarning>("import:warning", (e) => cb(e.payload));
}

export function onImportUnknowns(
  cb: (e: UnknownFile[]) => void,
): Promise<UnlistenFn> {
  return listen<UnknownFile[]>("import:unknowns", (e) => cb(e.payload));
}

export function onImportComplete(
  cb: (e: ImportResult) => void,
): Promise<UnlistenFn> {
  return listen<ImportResult>("import:complete", (e) => cb(e.payload));
}
