import { create } from "zustand";
import type { ChannelKind, GpsPoint, ScanError, Trip } from "../types/model";
import type {
  ImportSource,
  ImportPhaseChange,
  ImportProgress,
  ImportWarning,
  UnknownFile,
  ImportResult,
} from "../types/import";

export type AppStatus = "idle" | "loading" | "ready" | "error";

export interface LibrarySlice {
  trips: Trip[];
  selectedTripId: string | null;
  unmatched: string[];
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
  drift: { interior: number; rear: number };
  primaryChannel: ChannelKind;
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

  setStatus: (s: AppStatus) => void;
  setError: (e: string | null) => void;
  setVideoPort: (p: number | null) => void;
  setScanResult: (args: {
    trips: Trip[];
    unmatched: string[];
    errors: ScanError[];
  }) => void;
  selectTrip: (tripId: string | null) => void;
  setActiveSegmentId: (id: string | null) => void;
  setCurrentTime: (t: number) => void;
  setIsPlaying: (p: boolean) => void;
  setSpeed: (s: PlaybackSlice["speed"]) => void;
  setDrift: (d: { interior: number; rear: number }) => void;
  toggleDriftHud: () => void;
  setPrimaryChannel: (kind: ChannelKind) => void;
  setMultiChannelEnabled: (v: boolean) => void;
  toggleMultiChannelEnabled: () => void;
}

export const useStore = create<AppState>((set) => ({
  trips: [],
  selectedTripId: null,
  unmatched: [],
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
  drift: { interior: 0, rear: 0 },
  primaryChannel: "front",
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
  setScanResult: ({ trips, unmatched, errors }) =>
    set({
      trips,
      unmatched,
      scanErrors: errors,
      status: "ready",
      selectedTripId: trips[trips.length - 1]?.id ?? null,
    }),
  selectTrip: (tripId) =>
    set({
      selectedTripId: tripId,
      loadedTripId: tripId,
      activeSegmentId: null,
      currentTime: 0,
      isPlaying: false,
      primaryChannel: "front",
    }),
  setActiveSegmentId: (activeSegmentId) =>
    set({ activeSegmentId, currentTime: 0, isPlaying: false, primaryChannel: "front" }),
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
