export interface ImportSource {
  path: string;
  label: string;
  readOnly: boolean;
  fileCount: number;
  totalBytes: number;
}

export type ImportPhase =
  | "preflight"
  | "staging"
  | "wiping"
  | "distributing"
  | "cleanup";

export interface ImportPhaseChange {
  phase: ImportPhase;
  sourceLabel: string;
  message: string;
}

export interface ImportProgress {
  phase: ImportPhase;
  sourceLabel: string;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
  currentFile: string;
  speedBps: number;
}

export interface ImportWarning {
  message: string;
  sourceLabel: string;
}

export interface UnknownFile {
  stagedPath: string;
  relPath: string;
  extension: string;
  filename: string;
  size: number;
}

export type UnknownFileAction =
  | "deleteFilename"
  | "deleteExtension"
  | "moveToOther";

export interface UnknownFileDecision {
  stagedPath: string;
  action: UnknownFileAction;
}

export interface SourceResult {
  sourceLabel: string;
  sourcePath: string;
  filesStaged: number;
  bytesStaged: number;
  sourceWiped: boolean;
  readOnly: boolean;
  videosMoved: number;
  photosMoved: number;
  dupsSkipped: number;
  unknownFiles: number;
  noFiles: boolean;
  earliestDate: string | null;
  latestDate: string | null;
  error: string | null;
  warnings: string[];
}

export interface ImportResult {
  sources: SourceResult[];
  logPath: string | null;
}
