import type { ScanDoneEvent, ScanScope } from "../ipc/scanner";
import type { ScanProgress } from "../types/model";

/** Runtime state for the analysis-scan pipeline. */
export interface ScanSlice {
  scanRunning: boolean;
  scanStartTotal: number;
  /** Epoch ms when the current (or most recent) scan started. Used by
   *  the ScanView to compute a simple running-average ETA. */
  scanStartMs: number | null;
  scanProgress: ScanProgress | null;
  /** Set when `scan:done` fires; cleared when a new scan starts. */
  scanLastResult: ScanDoneEvent | null;

  startAnalysisScan: (scanIds: string[], scope: ScanScope) => Promise<void>;
  cancelAnalysisScan: () => Promise<void>;
}
