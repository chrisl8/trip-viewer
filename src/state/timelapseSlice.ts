import type {
  FfmpegCapabilities,
  StartTimelapseArgs,
  TimelapseDoneEvent,
  TimelapseJobRow,
  TimelapseProgressEvent,
} from "../ipc/timelapse";

/**
 * Runtime state for the timelapse generation pipeline. Mirrors the
 * shape of `ScanSlice` so the patterns are interchangeable.
 */
export interface TimelapseSlice {
  /** Whether a background encode is currently running. */
  timelapseRunning: boolean;
  /** Progress event from the last `timelapse:progress` tick. */
  timelapseProgress: TimelapseProgressEvent | null;
  /** Set when `timelapse:done` fires; cleared when a new run starts. */
  timelapseLastResult: TimelapseDoneEvent | null;
  /** Epoch ms when the current (or most recent) run started. Used for
   *  a simple running-average ETA in the view. */
  timelapseStartMs: number | null;

  /** All job rows from the DB, sorted newest-first. Refreshed on
   *  mount of the view and whenever a run completes. */
  timelapseJobs: TimelapseJobRow[];

  /** User-configured ffmpeg path, or null if not yet configured. */
  ffmpegPath: string | null;
  /** Cached capabilities of the configured ffmpeg. Null until the
   *  Test button has successfully probed the binary. */
  ffmpegCapabilities: FfmpegCapabilities | null;

  // Actions ────────────────────────────────────────────────────────────
  refreshTimelapseSettings: () => Promise<void>;
  refreshTimelapseJobs: () => Promise<void>;
  startTimelapseRun: (args: StartTimelapseArgs) => Promise<void>;
  cancelTimelapseRun: () => Promise<void>;
}
