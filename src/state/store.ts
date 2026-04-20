import { create } from "zustand";
import type { GpsPoint, ScanError, Trip } from "../types/model";
import type {
  ImportSource,
  ImportPhaseChange,
  ImportProgress,
  ImportWarning,
  UnknownFile,
  ImportResult,
} from "../types/import";

export type AppStatus = "idle" | "loading" | "ready" | "error";

export type MainView = "player" | "issues";

export interface LibrarySlice {
  trips: Trip[];
  selectedTripId: string | null;
  scanErrors: ScanError[];
  gpsByFile: Record<string, GpsPoint[]>;
}

export interface PlaybackSlice {
  loadedTripId: string | null;
  activeSegmentId: string | null;
  isPlaying: boolean;
  currentTime: number;
  speed: 0.5 | 1 | 2 | 4 | 8;
  volume: number;
  muted: boolean;
  showDriftHud: boolean;
  /** One entry per slave channel; label is the channel's free-form label. */
  drift: { label: string; driftMs: number }[];
  /** Label of the currently-primary channel, or null if no segment is
   *  loaded yet. Any string label is valid ("Front", "Interior", "Channel A", etc.). */
  primaryChannel: string | null;
  // Linux-only opt-in for rendering interior/rear channels. Off by default
  // on Linux because three concurrent HEVC pipelines can exhaust VRAM on
  // low-memory iGPUs (Vega 11 observed) and hang the GPU. Windows and macOS
  // ignore this and always render all three channels — see VideoGrid.tsx.
  multiChannelEnabled: boolean;
}

export type ImportStatus =
  | "idle"
  | "discovering"
  | "confirming"
  | "running"
  | "paused_unknowns"
  | "complete"
  | "error";

export interface ImportSlice {
  importStatus: ImportStatus;
  importSources: ImportSource[];
  importPhase: ImportPhaseChange | null;
  importProgress: ImportProgress | null;
  importWarnings: ImportWarning[];
  importUnknowns: UnknownFile[];
  importResult: ImportResult | null;
  importError: string | null;
  importRootPath: string | null;

  setImportStatus: (s: ImportStatus) => void;
  setImportSources: (sources: ImportSource[]) => void;
  setImportPhase: (phase: ImportPhaseChange | null) => void;
  setImportProgress: (progress: ImportProgress | null) => void;
  addImportWarning: (w: ImportWarning) => void;
  setImportUnknowns: (files: UnknownFile[]) => void;
  setImportResult: (result: ImportResult | null) => void;
  setImportError: (e: string | null) => void;
  setImportRootPath: (path: string | null) => void;
  resetImport: () => void;
}

export interface AppState extends LibrarySlice, PlaybackSlice, ImportSlice {
  status: AppStatus;
  error: string | null;
  videoPort: number | null;
  /** Which component fills the main pane right now. Reset to "player" on
   *  every new scan — loading a new folder should never strand the user
   *  on a stale issues list. */
  mainView: MainView;

  setStatus: (s: AppStatus) => void;
  setError: (e: string | null) => void;
  setVideoPort: (p: number | null) => void;
  setMainView: (v: MainView) => void;
  setScanResult: (args: {
    trips: Trip[];
    errors: ScanError[];
  }) => void;
  /** Remove scan errors whose paths are in the given set. Used to reflect
   *  successful deletions in the UI without re-running scan_folder. */
  removeScanErrors: (paths: string[]) => void;
  selectTrip: (tripId: string | null) => void;
  setActiveSegmentId: (id: string | null) => void;
  setCurrentTime: (t: number) => void;
  setIsPlaying: (p: boolean) => void;
  setSpeed: (s: PlaybackSlice["speed"]) => void;
  setDrift: (d: { label: string; driftMs: number }[]) => void;
  toggleDriftHud: () => void;
  setPrimaryChannel: (label: string | null) => void;
  setMultiChannelEnabled: (v: boolean) => void;
  toggleMultiChannelEnabled: () => void;
}

export const useStore = create<AppState>((set) => ({
  trips: [],
  selectedTripId: null,
  scanErrors: [],
  gpsByFile: {},

  loadedTripId: null,
  activeSegmentId: null,
  isPlaying: false,
  currentTime: 0,
  speed: 1,
  volume: 1,
  muted: false,
  showDriftHud: false,
  drift: [],
  // Primary channel is null until a segment is loaded; VideoGrid initializes
  // it to channels[0].label (the canonical master) on first render.
  primaryChannel: null,
  multiChannelEnabled: false,

  importStatus: "idle",
  importSources: [],
  importPhase: null,
  importProgress: null,
  importWarnings: [],
  importUnknowns: [],
  importResult: null,
  importError: null,
  importRootPath: null,

  status: "idle",
  error: null,
  videoPort: null,
  mainView: "player",

  setImportStatus: (importStatus) => set({ importStatus }),
  setImportSources: (importSources) => set({ importSources }),
  setImportPhase: (importPhase) => set({ importPhase }),
  setImportProgress: (importProgress) => set({ importProgress }),
  addImportWarning: (w) =>
    set((s) => ({ importWarnings: [...s.importWarnings, w] })),
  setImportUnknowns: (importUnknowns) =>
    set({ importUnknowns, importStatus: "paused_unknowns" }),
  setImportResult: (importResult) =>
    set({ importResult, importStatus: importResult ? "complete" : "idle" }),
  setImportError: (importError) =>
    set({ importError, importStatus: importError ? "error" : "idle" }),
  setImportRootPath: (importRootPath) => set({ importRootPath }),
  resetImport: () =>
    set({
      importStatus: "idle",
      importSources: [],
      importPhase: null,
      importProgress: null,
      importWarnings: [],
      importUnknowns: [],
      importResult: null,
      importError: null,
      importRootPath: null,
    }),

  setStatus: (status) => set({ status }),
  setError: (error) => set({ error, status: error ? "error" : "idle" }),
  setVideoPort: (videoPort) => set({ videoPort }),
  setMainView: (mainView) => set({ mainView }),
  setScanResult: ({ trips, errors }) =>
    set({
      trips,
      scanErrors: errors,
      status: "ready",
      selectedTripId: trips[trips.length - 1]?.id ?? null,
      mainView: "player",
    }),
  removeScanErrors: (paths) => {
    const drop = new Set(paths);
    set((s) => ({
      scanErrors: s.scanErrors.filter((e) => !drop.has(e.path)),
    }));
  },
  selectTrip: (tripId) =>
    set({
      selectedTripId: tripId,
      loadedTripId: tripId,
      activeSegmentId: null,
      currentTime: 0,
      isPlaying: false,
      // Reset to null; VideoGrid will set it to the new segment's master.
      primaryChannel: null,
      // Picking a trip implies the user wants to watch it — bail out of
      // the issues view if they were reading it.
      mainView: "player",
    }),
  setActiveSegmentId: (activeSegmentId) =>
    set({
      activeSegmentId,
      currentTime: 0,
      isPlaying: false,
      primaryChannel: null,
      mainView: "player",
    }),
  setCurrentTime: (currentTime) => set({ currentTime }),
  setIsPlaying: (isPlaying) => set({ isPlaying }),
  setSpeed: (speed) => set({ speed }),
  setDrift: (drift) => set({ drift }),
  toggleDriftHud: () => set((s) => ({ showDriftHud: !s.showDriftHud })),
  setPrimaryChannel: (primaryChannel) => set({ primaryChannel }),
  setMultiChannelEnabled: (multiChannelEnabled) => set({ multiChannelEnabled }),
  toggleMultiChannelEnabled: () =>
    set((s) => ({ multiChannelEnabled: !s.multiChannelEnabled })),
}));
