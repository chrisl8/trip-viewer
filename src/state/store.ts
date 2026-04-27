import { create } from "zustand";
import type { GpsPoint, ScanError, Tag, Trip } from "../types/model";
import type {
  ImportSource,
  ImportPhaseChange,
  ImportProgress,
  ImportWarning,
  UnknownFile,
  ImportResult,
} from "../types/import";
import type { TagsSlice } from "./tagsSlice";
import type { ScanSlice } from "./scanSlice";
import type { TimelapseSlice } from "./timelapseSlice";
import {
  getTagsForTrip,
  getTagCountsByTrip,
  listUserApplicableTags,
} from "../ipc/tags";
import { listPlaces } from "../ipc/places";
import {
  startAnalysisScan as ipcStartScan,
  cancelAnalysisScan as ipcCancelScan,
} from "../ipc/scanner";
import {
  cancelTimelapse as ipcCancelTimelapse,
  getTimelapseSettings as ipcGetTimelapseSettings,
  listTimelapseJobs as ipcListTimelapseJobs,
  startTimelapse as ipcStartTimelapse,
} from "../ipc/timelapse";
import type { CurveSegment } from "../utils/speedCurve";

export type AppStatus = "idle" | "loading" | "ready" | "error";

export type MainView =
  | "player"
  | "issues"
  | "scan"
  | "review"
  | "places"
  | "timelapse";

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
  /** Segment-local time in Original mode; file-time in tiered mode. */
  currentTime: number;
  // Browser playback-rate. 8x+ is handled by pre-rendered timelapse
  // files (selected via sourceMode) rather than <video>.playbackRate —
  // the browser's decoder stutters above 4x on multi-channel 4K.
  speed: 0.5 | 1 | 2 | 4;
  /** Playback source. "original" walks the segment stack as before.
   *  "8x" / "16x" / "60x" play a pre-rendered single-file-per-channel
   *  timelapse, with a speed curve mapping file-time ↔ concat-time. */
  sourceMode: "original" | "8x" | "16x" | "60x";
  /** The piecewise speed curve for the current (trip, tier), or null
   *  in Original mode. Loaded from `timelapse_jobs.speed_curve_json`
   *  when a tier is selected. */
  activeSpeedCurve: CurveSegment[] | null;
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

export interface AppState
  extends LibrarySlice,
    PlaybackSlice,
    ImportSlice,
    TagsSlice,
    ScanSlice,
    TimelapseSlice {
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
  /** Splice a segment out of the in-memory trips after the backend has
   *  removed it (delete-to-trash path). Drops the trip entirely if it
   *  becomes empty, advancing `selectedTripId` to the next trip. */
  removeSegmentFromTrip: (tripId: string, segmentId: string) => void;
  /** Batch version for multi-segment delete. Accepts a Set/Array of
   *  segment IDs and removes them all atomically (single store update). */
  removeSegmentsFromTrip: (tripId: string, segmentIds: string[]) => void;

  /** Whether the timeline is in multi-select mode. While on, segment
   *  clicks toggle selection instead of seeking. */
  selectionMode: boolean;
  /** IDs of segments currently selected in selection mode. */
  selectedSegmentIds: Set<string>;
  /** Last individually-clicked segment, used as the anchor for
   *  shift-click range selection. */
  selectionAnchorId: string | null;
  enterSelectionMode: () => void;
  exitSelectionMode: () => void;
  /** Toggle one segment's membership in the selection. Pass `range:true`
   *  to shift-click select from `selectionAnchorId` through `segmentId`
   *  using the in-memory order of the currently loaded trip. */
  toggleSegmentSelection: (
    segmentId: string,
    options?: { range?: boolean },
  ) => void;
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
  /** Switch the playback source. Pass `mode="original"` with a null
   *  curve to go back to the segment-walking stack. Passing a tier
   *  (`"8x"/"16x"/"60x"`) requires the caller to have already loaded
   *  the trip's speed curve for that tier. */
  setSourceMode: (
    mode: PlaybackSlice["sourceMode"],
    curve: CurveSegment[] | null,
  ) => void;
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
  sourceMode: "original",
  activeSpeedCurve: null,

  importStatus: "idle",
  importSources: [],
  importPhase: null,
  importProgress: null,
  importWarnings: [],
  importUnknowns: [],
  importResult: null,
  importError: null,
  importRootPath: null,

  tagsBySegmentId: {},
  tagsByTripId: {},
  tagsLoadingTripId: null,
  tripTagCounts: {},
  userApplicableTags: [],
  places: [],
  placesById: {},

  scanRunning: false,
  scanStartTotal: 0,
  scanStartMs: null,
  scanProgress: null,
  scanLastResult: null,

  timelapseRunning: false,
  timelapseProgress: null,
  timelapseLastResult: null,
  timelapseStartMs: null,
  timelapseJobs: [],
  ffmpegPath: null,
  ffmpegCapabilities: null,

  selectionMode: false,
  selectedSegmentIds: new Set<string>(),
  selectionAnchorId: null,

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
  setScanResult: ({ trips, errors }) => {
    set({
      trips,
      scanErrors: errors,
      status: "ready",
      selectedTripId: trips[trips.length - 1]?.id ?? null,
      mainView: "player",
    });
    // Fresh folder scan means tags may have been GC'd or the trip set
    // changed — reload aggregate counts so sidebar badges stay honest.
    void useStore.getState().refreshTripTagCounts();
  },
  removeScanErrors: (paths) => {
    const drop = new Set(paths);
    set((s) => ({
      scanErrors: s.scanErrors.filter((e) => !drop.has(e.path)),
    }));
  },
  removeSegmentFromTrip: (tripId, segmentId) =>
    useStore.getState().removeSegmentsFromTrip(tripId, [segmentId]),
  removeSegmentsFromTrip: (tripId, segmentIds) =>
    set((s) => {
      const tripIdx = s.trips.findIndex((t) => t.id === tripId);
      if (tripIdx < 0) return {};
      const trip = s.trips[tripIdx];
      const dropSet = new Set(segmentIds);
      const remaining = trip.segments.filter((seg) => !dropSet.has(seg.id));
      const nextTrips = [...s.trips];
      if (remaining.length === 0) {
        // Trip is now empty; drop it from the list and advance the
        // selection to the next adjacent trip (preferring the one that
        // was after this one, falling back to the previous, then null).
        nextTrips.splice(tripIdx, 1);
        let nextSelected: string | null = s.selectedTripId;
        if (s.selectedTripId === tripId) {
          nextSelected =
            nextTrips[tripIdx]?.id ?? nextTrips[tripIdx - 1]?.id ?? null;
        }
        return {
          trips: nextTrips,
          selectedTripId: nextSelected,
          loadedTripId:
            s.loadedTripId === tripId ? nextSelected : s.loadedTripId,
        };
      }
      // Trip still has segments; rewrite it in place.
      nextTrips[tripIdx] = { ...trip, segments: remaining };
      return { trips: nextTrips };
    }),

  enterSelectionMode: () =>
    set({
      selectionMode: true,
      selectedSegmentIds: new Set<string>(),
      selectionAnchorId: null,
      // Pause playback so the user isn't fighting auto-advance while
      // building a selection.
      isPlaying: false,
    }),
  exitSelectionMode: () =>
    set({
      selectionMode: false,
      selectedSegmentIds: new Set<string>(),
      selectionAnchorId: null,
    }),
  toggleSegmentSelection: (segmentId, options) =>
    set((s) => {
      const next = new Set(s.selectedSegmentIds);
      if (options?.range && s.selectionAnchorId) {
        // Shift-click range: take every segment between anchor and the
        // clicked one (inclusive) in the loaded trip's order. Always
        // *adds* — never deselects — so a careless shift-click can't
        // wipe the prior selection.
        const trip = s.trips.find((t) => t.id === s.loadedTripId);
        if (trip) {
          const anchorIdx = trip.segments.findIndex(
            (seg) => seg.id === s.selectionAnchorId,
          );
          const clickedIdx = trip.segments.findIndex(
            (seg) => seg.id === segmentId,
          );
          if (anchorIdx >= 0 && clickedIdx >= 0) {
            const lo = Math.min(anchorIdx, clickedIdx);
            const hi = Math.max(anchorIdx, clickedIdx);
            for (let i = lo; i <= hi; i++) {
              next.add(trip.segments[i].id);
            }
            return {
              selectedSegmentIds: next,
              selectionAnchorId: segmentId,
            };
          }
        }
      }
      // Plain click: toggle this one segment, set as new anchor.
      if (next.has(segmentId)) next.delete(segmentId);
      else next.add(segmentId);
      return {
        selectedSegmentIds: next,
        selectionAnchorId: segmentId,
      };
    }),
  selectTrip: (tripId) => {
    set({
      selectedTripId: tripId,
      loadedTripId: tripId,
      activeSegmentId: null,
      currentTime: 0,
      isPlaying: false,
      // Reset to null; VideoGrid will set it to the new segment's master.
      primaryChannel: null,
      // Trip change always returns to Original source. Each trip has
      // its own speed curves (different tiers, different event windows).
      sourceMode: "original",
      activeSpeedCurve: null,
      // Picking a trip implies the user wants to watch it — bail out of
      // the issues view if they were reading it.
      mainView: "player",
      // Selections are scoped to a single trip; abandon when navigating
      // away so the user can't accidentally delete cross-trip.
      selectionMode: false,
      selectedSegmentIds: new Set<string>(),
      selectionAnchorId: null,
    });
    if (tripId) {
      void useStore.getState().refreshTripTags(tripId);
    } else {
      useStore.getState().clearTags();
    }
  },
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
  /** Switch between Original and a pre-rendered tier. The caller is
   *  responsible for converting the current playback position into
   *  the new mode's time axis *before* calling this — that extra
   *  trip-time/seek choreography lives in the SourceControls UI and
   *  in PlayerShell's seekTripTime helper. This action just swaps
   *  the flags atomically. */
  setSourceMode: (sourceMode, activeSpeedCurve) =>
    set({ sourceMode, activeSpeedCurve }),

  refreshTripTags: async (tripId) => {
    set({ tagsLoadingTripId: tripId });
    try {
      const tags = await getTagsForTrip(tripId);
      const tripTags: Tag[] = [];
      const bySegment: Record<string, Tag[]> = {};
      for (const tag of tags) {
        if (tag.segmentId) {
          (bySegment[tag.segmentId] ??= []).push(tag);
        } else if (tag.tripId) {
          tripTags.push(tag);
        }
      }
      set({
        tagsBySegmentId: bySegment,
        tagsByTripId: { [tripId]: tripTags },
        tagsLoadingTripId: null,
      });
    } catch (e) {
      console.error("refreshTripTags failed", e);
      set({ tagsLoadingTripId: null });
    }
  },
  refreshTripTagCounts: async () => {
    try {
      const counts = await getTagCountsByTrip();
      set({ tripTagCounts: counts });
    } catch (e) {
      console.error("refreshTripTagCounts failed", e);
    }
  },
  loadUserApplicableTags: async () => {
    try {
      const tags = await listUserApplicableTags();
      set({ userApplicableTags: tags });
    } catch (e) {
      console.error("loadUserApplicableTags failed", e);
    }
  },
  refreshPlaces: async () => {
    try {
      const places = await listPlaces();
      const placesById: Record<number, (typeof places)[number]> = {};
      for (const p of places) placesById[p.id] = p;
      set({ places, placesById });
    } catch (e) {
      console.error("refreshPlaces failed", e);
    }
  },

  startAnalysisScan: async (scanIds, scope) => {
    set({
      scanRunning: true,
      scanStartTotal: 0,
      scanProgress: null,
      scanLastResult: null,
    });
    try {
      await ipcStartScan(scanIds, scope);
    } catch (e) {
      console.error("startAnalysisScan failed", e);
      set({ scanRunning: false });
      throw e;
    }
  },
  cancelAnalysisScan: async () => {
    await ipcCancelScan();
  },

  refreshTimelapseSettings: async () => {
    try {
      const s = await ipcGetTimelapseSettings();
      set({
        ffmpegPath: s.ffmpegPath,
        ffmpegCapabilities: s.capabilities,
      });
    } catch (e) {
      console.error("refreshTimelapseSettings failed", e);
    }
  },
  refreshTimelapseJobs: async () => {
    try {
      const jobs = await ipcListTimelapseJobs();
      set({ timelapseJobs: jobs });
    } catch (e) {
      console.error("refreshTimelapseJobs failed", e);
    }
  },
  startTimelapseRun: async (args) => {
    set({
      timelapseRunning: true,
      timelapseProgress: null,
      timelapseLastResult: null,
    });
    try {
      await ipcStartTimelapse(args);
    } catch (e) {
      console.error("startTimelapseRun failed", e);
      set({ timelapseRunning: false });
      throw e;
    }
  },
  cancelTimelapseRun: async () => {
    await ipcCancelTimelapse();
  },

  clearTags: () =>
    set({
      tagsBySegmentId: {},
      tagsByTripId: {},
      tagsLoadingTripId: null,
      tripTagCounts: {},
    }),
  // places are NOT cleared here — they're library-wide, not per-trip.
}));
