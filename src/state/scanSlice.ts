import type {
  ScanDoneEvent,
  ScanScope,
  TripScanCoverage,
} from "../ipc/scanner";
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
  /** Per-trip × per-scan coverage matrix for the Scan view's Trips
   *  table. Refreshed on view mount and polled while a scan runs. */
  scanCoverage: TripScanCoverage[];

  startAnalysisScan: (
    scanIds: string[],
    scope: ScanScope,
    tripIds?: string[] | null,
  ) => Promise<void>;
  cancelAnalysisScan: () => Promise<void>;
  refreshScanCoverage: () => Promise<void>;
}
