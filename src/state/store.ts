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
  deleteSegmentsToTrash as ipcDeleteSegmentsToTrash,
  getTagsForTrip,
  getTagCountsByTrip,
  listUserApplicableTags,
  type DeleteReport,
} from "../ipc/tags";
import { listPlaces } from "../ipc/places";
import {
  assessTripMerge as ipcAssessTripMerge,
  deleteTrip as ipcDeleteTrip,
  listArchiveOnlyTrips,
  mergeTrips as ipcMergeTrips,
  type DeleteTripReport,
  type MergeReport,
  type TimelapseMergeAssessment,
  type TimelapseMergeStrategy,
} from "../ipc/trips";
import {
  startAnalysisScan as ipcStartScan,
  cancelAnalysisScan as ipcCancelScan,
  listScanCoverage as ipcListScanCoverage,
} from "../ipc/scanner";
import {
  cancelTimelapse as ipcCancelTimelapse,
  countOrphanTimelapseFiles as ipcCountOrphanTimelapseFiles,
  getTimelapseSettings as ipcGetTimelapseSettings,
  listTimelapseJobs as ipcListTimelapseJobs,
  startTimelapse as ipcStartTimelapse,
} from "../ipc/timelapse";
import {
  getLibraryStorageSummary as ipcGetLibraryStorageSummary,
  type LibraryStorageSummary,
} from "../ipc/library";
import { loadTripGps as ipcLoadTripGps } from "../ipc/gps";
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
  /** Trip-stitched GPS loaded from the DB at trip-select time. Present
   *  for trips whose timelapse encode has persisted GPS via migration
   *  0012's `trip_gps` table. Absent key → fall back to gpsByFile
   *  (which requires the originals to still be on disk). */
  tripGpsByTrip: Record<string, GpsPoint[]>;
  gpsPreparationByTrip: Record<string, "loading" | "ready" | "error">;
  gpsProgressByTrip: Record<string, { completed: number; total: number }>;
  /** Library-wide bytes used + reclaimable, refreshed after every
   *  storage-changing action. `null` until the first refresh resolves. */
  librarySummary: LibraryStorageSummary | null;
  /** When true, TripList shows only trips whose ids are in
   *  `librarySummary.reclaimableTripIds`. Toggled by clicking the
   *  reclaimable count in the sidebar header. */
  reclaimableFilter: boolean;
  /** False until the first auto-scan (or archive-only merge) on app
   *  start has resolved. The sidebar trip list and welcome panel show
   *  a "Loading library…" placeholder while this is false so the user
   *  never sees the brief intermediate state where only archive-only
   *  trips have landed but the folder scan is still running. */
  libraryFirstLoadDone: boolean;
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
  /** Channel labels currently inside a coverage gap (the camera was off
   *  for this stretch of the trip). In tiered playback the SyncEngine
   *  holds that channel's `<video>` and `ChannelPanel` draws a black
   *  overlay. Empty in Original mode and for full-coverage channels. */
  gappedChannels: Record<string, boolean>;
  volume: number;
  muted: boolean;
  showDriftHud: boolean;
  /** One entry per slave channel; label is the channel's free-form label. */
  drift: { label: string; driftMs: number }[];
  /** Label of the currently-primary channel, or null if no segment is
   *  loaded yet. Any string label is valid ("Front", "Interior", "Channel A", etc.). */
  primaryChannel: string | null;
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

/** Active archive snapshot. `null` means no archive is currently open
 *  and the frontend should show the empty state. The store mirrors what
 *  the backend's `current_archive` command would return — refreshed
 *  after `openArchive` / `closeArchive` and on app mount. */
export interface CurrentArchive {
  root: string;
  label: string;
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
  currentArchive: CurrentArchive | null;

  setStatus: (s: AppStatus) => void;
  setError: (e: string | null) => void;
  setVideoPort: (p: number | null) => void;
  setMainView: (v: MainView) => void;
  setCurrentArchive: (a: CurrentArchive | null) => void;
  setScanResult: (args: {
    trips: Trip[];
    errors: ScanError[];
  }) => void;
  /** Remove scan errors whose paths are in the given set. Used to reflect
   *  successful deletions in the UI without re-running scan_folder. */
  removeScanErrors: (paths: string[]) => void;
  /** Splice a segment out of the in-memory trips after the backend has
   *  removed it (delete-to-trash path). Drops the trip entirely if it
   *  becomes empty *and* has no timelapse archive; otherwise the trip
   *  is left in place with `archiveOnly: true` so it stays discoverable. */
  removeSegmentFromTrip: (tripId: string, segmentId: string) => void;
  /** Batch version for multi-segment delete. Accepts the IDs the
   *  backend operated on plus the subset that were converted to
   *  tombstones (kept in the trip with `isTombstone: true` so the
   *  timeline renders a hatched gap). The rest are hard-spliced. */
  removeSegmentsFromTrip: (
    tripId: string,
    segmentIds: string[],
    tombstonedIds?: string[],
  ) => void;
  /** Fetch archive-only trips from the DB and merge them into `trips`,
   *  sorted by startTime. Idempotent — re-running just refreshes the
   *  archive set without disturbing scanned trips. */
  mergeArchiveOnlyTrips: () => Promise<void>;
  /** Trash every source MP4 for a trip while keeping the timelapse
   *  archive intact. Convenience for the trip-level "Delete originals…"
   *  action — calls the existing per-segment IPC under the hood with
   *  every segment ID for the trip, then updates the in-memory trips
   *  to leave it as archive-only. */
  deleteOriginalsForTrip: (tripId: string) => Promise<DeleteReport>;
  /** Wholesale "delete this trip" — sources, timelapse pre-renders,
   *  and DB rows. Removes the trip from the in-memory list on success
   *  and advances the selection to the next adjacent trip. The only
   *  store action that ever causes a timelapse archive to be removed. */
  deleteTripCompletely: (tripId: string) => Promise<DeleteTripReport>;
  /** Re-fetch the library storage summary (totals + reclaimable trip
   *  ids). Called after any action that changes segment or timelapse
   *  bytes on disk. Errors are swallowed — a stale total is better
   *  than a crashed sidebar. */
  refreshLibrarySummary: () => Promise<void>;
  /** Re-fetch a trip's persisted (trip-stitched) GPS into
   *  `tripGpsByTrip`. Called after a rebuild so the map + speed graph
   *  populate without the user having to close and reopen the trip
   *  (the encoder persists fresh GPS during the rebuild). */
  refreshTripGps: (tripId: string) => Promise<void>;
  /** Toggle the reclaimable-only filter in the sidebar trip list. */
  setReclaimableFilter: (enabled: boolean) => void;

  /** IDs of trips the user has multi-selected in the sidebar via
   *  Ctrl/Cmd-click (toggle one) or Shift-click (range). Session-scoped —
   *  not persisted to disk. Drives the selection toolbar's group actions
   *  (merge, bulk delete). Cleared after a successful group action, on
   *  explicit "Clear", or when a plain click opens a single trip. */
  selectedTripIds: Set<string>;
  /** Anchor row for Shift-click range selection in the trip list. */
  tripSelectionAnchorId: string | null;
  /** Toggle a trip's membership in the multi-selection. Pass
   *  `range:true` with the current visible `order` (trip ids top-to-bottom)
   *  to select the span from `tripSelectionAnchorId` through `tripId`
   *  inclusive. Range always *adds* — never deselects. */
  toggleTripSelection: (
    tripId: string,
    options?: { range?: boolean; order?: string[] },
  ) => void;
  clearTripSelection: () => void;
  /** Bulk "Delete originals…" across the selected trips. Trashes source
   *  files but keeps timelapse archives. Clears the selection on completion
   *  and returns the per-trip failures so the dialog can surface them. */
  deleteOriginalsForTrips: (
    tripIds: string[],
  ) => Promise<{ failures: { tripId: string; message: string }[] }>;
  /** Bulk "Delete trip…" across the selected trips. Trashes originals AND
   *  timelapse archives, removing the trips from the library. */
  deleteTripsCompletely: (
    tripIds: string[],
  ) => Promise<{ failures: { tripId: string; message: string }[] }>;
  /** Read-only assessment of what's possible for the selected trips'
   *  existing timelapse outputs. Returns null when nothing is selected
   *  or only one is selected (need 2+ for a merge). */
  assessMergeMarked: () => Promise<TimelapseMergeAssessment | null>;
  /** Perform the merge. Returns the backend report. The primary is
   *  chosen as the earliest-starting selected trip (its segments come
   *  first in the merged timeline). On success, the in-memory trip
   *  list is rewritten to reflect the merge and `selectedTripIds` is
   *  cleared. */
  mergeMarkedTrips: (strategy: TimelapseMergeStrategy) => Promise<MergeReport>;

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
  /** Switch the playback source. Pass `mode="original"` with a null
   *  curve to go back to the segment-walking stack. Passing a tier
   *  (`"8x"/"16x"/"60x"`) requires the caller to have already loaded
   *  the trip's speed curve for that tier. */
  setSourceMode: (
    mode: PlaybackSlice["sourceMode"],
    curve: CurveSegment[] | null,
  ) => void;
  /** Mark/unmark a channel as currently gapped (SyncEngine drives this). */
  setChannelGapped: (label: string, gapped: boolean) => void;
}

export const useStore = create<AppState>((set) => ({
  trips: [],
  selectedTripId: null,
  scanErrors: [],
  gpsByFile: {},
  tripGpsByTrip: {},
  gpsPreparationByTrip: {},
  gpsProgressByTrip: {},
  librarySummary: null,
  reclaimableFilter: false,
  libraryFirstLoadDone: false,
  selectedTripIds: new Set<string>(),
  tripSelectionAnchorId: null,

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
  sourceMode: "original",
  activeSpeedCurve: null,
  gappedChannels: {},

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
  scanCoverage: [],

  timelapseRunning: false,
  timelapseScanning: false,
  timelapseProgress: null,
  timelapseLastResult: null,
  timelapseStartMs: null,
  timelapseJobs: [],
  orphanTimelapseCount: 0,
  ffmpegPath: null,
  ffmpegCapabilities: null,

  selectionMode: false,
  selectedSegmentIds: new Set<string>(),
  selectionAnchorId: null,

  status: "idle",
  error: null,
  videoPort: null,
  mainView: "player",
  currentArchive: null,

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
  setError: (error) =>
    set({
      error,
      status: error ? "error" : "idle",
      // Unstick the "Loading library…" placeholder so the user sees
      // the error / empty state instead of an indefinite spinner.
      ...(error ? { libraryFirstLoadDone: true } : {}),
    }),
  setVideoPort: (videoPort) => set({ videoPort }),
  setMainView: (mainView) => set({ mainView }),
  setCurrentArchive: (currentArchive) => set({
    currentArchive: currentArchive ? { ...currentArchive } : null,
    status: currentArchive ? "loading" : "idle",
    error: null,
    libraryFirstLoadDone: currentArchive === null,
    trips: [], scanErrors: [], selectedTripId: null, loadedTripId: null,
    activeSegmentId: null, currentTime: 0, isPlaying: false, primaryChannel: null,
    sourceMode: "original", activeSpeedCurve: null, gappedChannels: {}, drift: [],
    gpsByFile: {}, tripGpsByTrip: {}, gpsPreparationByTrip: {}, gpsProgressByTrip: {},
    librarySummary: null, reclaimableFilter: false,
    selectedTripIds: new Set<string>(), tripSelectionAnchorId: null,
    selectionMode: false, selectedSegmentIds: new Set<string>(), selectionAnchorId: null,
    tagsBySegmentId: {}, tagsByTripId: {}, tagsLoadingTripId: null, tripTagCounts: {},
    places: [], placesById: {}, timelapseJobs: [], orphanTimelapseCount: 0,
    scanCoverage: [], mainView: "player",
  }),
  setScanResult: ({ trips, errors }) => {
    const archive = useStore.getState().currentArchive;
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
    // Merge archive-only trips (segments gone, timelapse remains) into
    // the trip list. Done after the scan-result set so the user sees
    // their freshly-scanned trips immediately, with archive-only ones
    // sliding in once the DB query returns. Sorted by startTime so they
    // interleave correctly. Flip `libraryFirstLoadDone` only after the
    // merge resolves — otherwise the sidebar briefly shows "No trips
    // loaded" between setScanResult (which clears the placeholder)
    // and mergeArchiveOnlyTrips landing the archive-only entries.
    void useStore
      .getState()
      .mergeArchiveOnlyTrips()
      .finally(() => {
        if (useStore.getState().currentArchive === archive) {
          useStore.setState({ libraryFirstLoadDone: true });
        }
      });
    // Sizes change on every scan (new segments, vanished segments,
    // backfilled sizes from migration 0009) — refresh the header
    // summary so "X GB used / Y GB reclaimable" stays honest.
    void useStore.getState().refreshLibrarySummary();
  },
  removeScanErrors: (paths) => {
    const drop = new Set(paths);
    set((s) => ({
      scanErrors: s.scanErrors.filter((e) => !drop.has(e.path)),
    }));
  },
  removeSegmentFromTrip: (tripId, segmentId) =>
    useStore.getState().removeSegmentsFromTrip(tripId, [segmentId]),
  removeSegmentsFromTrip: (tripId, segmentIds, tombstonedIds) => {
    // Schedule a backend reconcile right after the synchronous local
    // update. The local rule (keep trip if `timelapseJobs` has any row
    // for it) is correct only when the frontend's job cache is in
    // sync; the merge is the source of truth and re-adds the trip as
    // archive-only if the user hasn't visited the Timelapse view yet.
    const reconcile = () => {
      void useStore.getState().mergeArchiveOnlyTrips();
      // Originals just got trashed → reclaimable bytes drop, total
      // bytes drop. Refresh the header summary alongside the reconcile.
      void useStore.getState().refreshLibrarySummary();
    };
    setTimeout(reconcile, 0);
    const tombstoneSet = new Set(tombstonedIds ?? []);
    set((s) => {
      const tripIdx = s.trips.findIndex((t) => t.id === tripId);
      if (tripIdx < 0) return {};
      const trip = s.trips[tripIdx];
      const opSet = new Set(segmentIds);
      // Build the next segment list:
      //  - tombstoned IDs stay in place but get `isTombstone: true`
      //    and an empty channels array (the originals are gone).
      //  - other operated-on IDs get spliced (hard-deleted upstream).
      //  - untouched segments pass through.
      const nextSegments = trip.segments
        .map((seg) => {
          if (!opSet.has(seg.id)) return seg;
          if (tombstoneSet.has(seg.id)) {
            return {
              ...seg,
              channels: [],
              isTombstone: true,
              sizeBytes: null,
            };
          }
          return null;
        })
        .filter((seg): seg is (typeof trip.segments)[number] => seg !== null);
      const survivingNonTombstone = nextSegments.filter((seg) => !seg.isTombstone);
      const nextTrips = [...s.trips];
      if (survivingNonTombstone.length === 0) {
        // No real segments left. Backend has hard-deleted any tombstones
        // it just created (so the trip can flip cleanly to archive-only).
        // If there's a timelapse archive, keep the trip in place with
        // empty segments + archiveOnly; otherwise drop it entirely.
        const hasArchive = s.timelapseJobs.some((j) => j.tripId === tripId);
        if (hasArchive) {
          nextTrips[tripIdx] = {
            ...trip,
            segments: [],
            archiveOnly: true,
          };
          return { trips: nextTrips };
        }
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
      // Mix of survivors + (possibly) tombstones. Trip stays as a
      // normal trip; the timeline will render hatched gaps for the
      // tombstones based on `isTombstone`.
      nextTrips[tripIdx] = { ...trip, segments: nextSegments };
      return { trips: nextTrips };
    });
  },
  deleteOriginalsForTrip: async (tripId) => {
    const trip = useStore.getState().trips.find((t) => t.id === tripId);
    if (!trip) {
      return {
        segmentsRemoved: 0,
        filesTrashed: 0,
        failures: [],
        tombstonedSegmentIds: [],
      };
    }
    const segmentIds = trip.segments.map((s) => s.id);
    const inMemoryPaths: Record<string, string[]> = {};
    for (const seg of trip.segments) {
      inMemoryPaths[seg.id] = seg.channels
        .map((c) => c.filePath)
        .filter((p): p is string => Boolean(p));
    }
    const report = await ipcDeleteSegmentsToTrash(segmentIds, inMemoryPaths);
    // Splice out everything that didn't fail. Mirrors the per-segment
    // path's optimistic update; survivors stay in the list and the
    // failure list surfaces in the dialog so the user can retry.
    const failedPaths = new Set(report.failures.map((f) => f.path));
    const survivors = new Set<string>();
    for (const segId of segmentIds) {
      const paths = inMemoryPaths[segId] ?? [];
      if (paths.length > 0 && paths.every((p) => failedPaths.has(p))) {
        survivors.add(segId);
      }
    }
    const removed = segmentIds.filter((id) => !survivors.has(id));
    if (removed.length > 0) {
      useStore
        .getState()
        .removeSegmentsFromTrip(tripId, removed, report.tombstonedSegmentIds);
    }
    return report;
  },
  deleteTripCompletely: async (tripId) => {
    const trip = useStore.getState().trips.find((t) => t.id === tripId);
    const inMemoryPaths: Record<string, string[]> = {};
    if (trip) {
      for (const seg of trip.segments) {
        inMemoryPaths[seg.id] = seg.channels
          .map((c) => c.filePath)
          .filter((p): p is string => Boolean(p));
      }
    }
    const report = await ipcDeleteTrip(tripId, inMemoryPaths);
    if (report.tripRemoved) {
      // Both originals and timelapses got trashed — refresh totals.
      void useStore.getState().refreshLibrarySummary();
      // Remove the trip from the local list and advance selection.
      // This is the only path that ever fully removes a trip with a
      // timelapse archive; the archive-only fallback in
      // `removeSegmentsFromTrip` doesn't apply here because we just
      // deleted the timelapse_jobs rows on the backend.
      useStore.setState((s) => {
        const idx = s.trips.findIndex((t) => t.id === tripId);
        if (idx < 0) return {};
        const nextTrips = [...s.trips];
        nextTrips.splice(idx, 1);
        let nextSelected: string | null = s.selectedTripId;
        if (s.selectedTripId === tripId) {
          nextSelected =
            nextTrips[idx]?.id ?? nextTrips[idx - 1]?.id ?? null;
        }
        // Also drop the timelapse_jobs cache entries for this trip so
        // the player and TimelapseView don't briefly show stale rows
        // before the next refresh.
        const nextJobs = s.timelapseJobs.filter((j) => j.tripId !== tripId);
        return {
          trips: nextTrips,
          selectedTripId: nextSelected,
          loadedTripId:
            s.loadedTripId === tripId ? nextSelected : s.loadedTripId,
          timelapseJobs: nextJobs,
        };
      });
    }
    return report;
  },
  mergeArchiveOnlyTrips: async () => {
    const current = useStore.getState().currentArchive;
    try {
      const archive = await listArchiveOnlyTrips();
      if (useStore.getState().currentArchive !== current) return;
      if (archive.length === 0) {
        // Still drop any stale archive-only entries that are no longer
        // backed by a row in the DB (e.g. after Delete trip…).
        useStore.setState((s) => {
          const stripped = s.trips.filter((t) => !t.archiveOnly);
          return stripped.length === s.trips.length ? {} : { trips: stripped };
        });
        return;
      }
      useStore.setState((s) => {
        // Drop any existing archive-only trips, then merge the fresh
        // set in. Trips that the scan returned (with segments) take
        // precedence — never overwrite a scanned trip with an
        // archive-only entry of the same id.
        const scanned = s.trips.filter((t) => !t.archiveOnly);
        const scannedIds = new Set(scanned.map((t) => t.id));
        const merged = [
          ...scanned,
          ...archive.filter((t) => !scannedIds.has(t.id)),
        ];
        merged.sort((a, b) => a.startTime.localeCompare(b.startTime));
        return { trips: merged };
      });
    } catch (e) {
      console.error("mergeArchiveOnlyTrips failed", e);
    }
  },

  toggleTripSelection: (tripId, options) =>
    useStore.setState((s) => {
      const next = new Set(s.selectedTripIds);
      if (options?.range && s.tripSelectionAnchorId && options.order) {
        // Shift-click range: select every trip between the anchor and the
        // clicked one (inclusive) in the *visible* order passed by the
        // list. Always adds — never deselects — so a stray shift-click
        // can't wipe an existing selection.
        const order = options.order;
        const anchorIdx = order.indexOf(s.tripSelectionAnchorId);
        const clickedIdx = order.indexOf(tripId);
        if (anchorIdx >= 0 && clickedIdx >= 0) {
          const lo = Math.min(anchorIdx, clickedIdx);
          const hi = Math.max(anchorIdx, clickedIdx);
          for (let i = lo; i <= hi; i++) next.add(order[i]);
          return { selectedTripIds: next, tripSelectionAnchorId: tripId };
        }
      }
      // Plain toggle (Ctrl/Cmd-click): flip this one, set it as the new
      // anchor for any subsequent shift-click range.
      if (next.has(tripId)) next.delete(tripId);
      else next.add(tripId);
      return { selectedTripIds: next, tripSelectionAnchorId: tripId };
    }),

  clearTripSelection: () =>
    useStore.setState({
      selectedTripIds: new Set<string>(),
      tripSelectionAnchorId: null,
    }),

  deleteOriginalsForTrips: async (tripIds) => {
    const failures: { tripId: string; message: string }[] = [];
    // Sequential — each trip's delete optimistically rewrites the trip
    // list, and running them serially keeps those updates from racing.
    for (const tripId of tripIds) {
      try {
        const report =
          await useStore.getState().deleteOriginalsForTrip(tripId);
        if (report.failures.length > 0) {
          failures.push({
            tripId,
            message:
              report.failures.length === 1
                ? report.failures[0].message
                : `${report.failures.length} files could not be moved to trash`,
          });
        }
      } catch (e) {
        failures.push({
          tripId,
          message: e instanceof Error ? e.message : String(e),
        });
      }
    }
    useStore.getState().clearTripSelection();
    return { failures };
  },

  deleteTripsCompletely: async (tripIds) => {
    const failures: { tripId: string; message: string }[] = [];
    for (const tripId of tripIds) {
      try {
        const report = await useStore.getState().deleteTripCompletely(tripId);
        if (report.failures.length > 0) {
          failures.push({
            tripId,
            message:
              report.failures.length === 1
                ? report.failures[0].message
                : `${report.failures.length} files could not be moved to trash`,
          });
        }
      } catch (e) {
        failures.push({
          tripId,
          message: e instanceof Error ? e.message : String(e),
        });
      }
    }
    useStore.getState().clearTripSelection();
    return { failures };
  },

  assessMergeMarked: async () => {
    const state = useStore.getState();
    if (state.selectedTripIds.size < 2) return null;
    // Earliest-start wins — the resulting merged trip's UUID matches
    // the natural earliest trip's, so its segments come first in any
    // chronological view and the player resumes there naturally.
    const marked: Trip[] = state.trips.filter((t) =>
      state.selectedTripIds.has(t.id),
    );
    marked.sort((a, b) => a.startTime.localeCompare(b.startTime));
    const primaryId = marked[0]?.id;
    const absorbedIds = marked.slice(1).map((t) => t.id);
    if (!primaryId || absorbedIds.length === 0) return null;
    return ipcAssessTripMerge(primaryId, absorbedIds);
  },

  mergeMarkedTrips: async (strategy) => {
    const state = useStore.getState();
    if (state.selectedTripIds.size < 2) {
      throw new Error("Need at least 2 trips selected to merge");
    }
    const marked: Trip[] = state.trips.filter((t) =>
      state.selectedTripIds.has(t.id),
    );
    marked.sort((a, b) => a.startTime.localeCompare(b.startTime));
    const primaryId = marked[0].id;
    const absorbedIds = new Set(marked.slice(1).map((t) => t.id));
    const report = await ipcMergeTrips(
      primaryId,
      Array.from(absorbedIds),
      strategy,
    );

    // Local update: fold absorbed segments into primary, drop absorbed
    // trip rows. The backend has already done this in the DB; we
    // mirror it locally so the UI updates without a full rescan.
    useStore.setState((s) => {
      const nextTrips: Trip[] = [];
      let mergedTrip: Trip | undefined;
      for (const t of s.trips) {
        if (t.id === primaryId) {
          mergedTrip = { ...t };
          nextTrips.push(mergedTrip);
        } else if (!absorbedIds.has(t.id)) {
          nextTrips.push(t);
        }
        // Absorbed trips are dropped; their segments fold below.
      }
      if (mergedTrip) {
        const allSegs = [
          ...mergedTrip.segments,
          ...marked.slice(1).flatMap((t) => t.segments),
        ];
        allSegs.sort((a, b) => a.startTime.localeCompare(b.startTime));
        mergedTrip.segments = allSegs;
        if (allSegs.length > 0) {
          mergedTrip.startTime = allSegs[0].startTime;
          const last = allSegs[allSegs.length - 1];
          const lastEnd =
            new Date(last.startTime).getTime() + (last.durationS ?? 0) * 1000;
          mergedTrip.endTime = new Date(lastEnd).toISOString();
        }
        mergedTrip.archiveOnly = allSegs.length === 0;
      }
      // Drop timelapse jobs for absorbed trips — backend either
      // deleted them or rewrote them under primary's trip_id.
      const nextJobs = s.timelapseJobs.filter(
        (j) => !absorbedIds.has(j.tripId),
      );
      const nextSelected =
        s.selectedTripId && absorbedIds.has(s.selectedTripId)
          ? primaryId
          : s.selectedTripId;
      const nextLoaded =
        s.loadedTripId && absorbedIds.has(s.loadedTripId)
          ? primaryId
          : s.loadedTripId;
      return {
        trips: nextTrips,
        selectedTripIds: new Set<string>(),
        tripSelectionAnchorId: null,
        timelapseJobs: nextJobs,
        selectedTripId: nextSelected,
        loadedTripId: nextLoaded,
      };
    });

    // Refresh timelapse jobs from DB to pick up any concat-produced
    // primary rows the local update doesn't know about.
    void useStore.getState().refreshTimelapseJobs();
    return report;
  },

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
      gappedChannels: {},
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
      gappedChannels: {},
      mainView: "player",
    }),
  setCurrentTime: (currentTime) => set({ currentTime }),
  setIsPlaying: (isPlaying) => set({ isPlaying }),
  setSpeed: (speed) => set({ speed }),
  setDrift: (drift) => set({ drift }),
  toggleDriftHud: () => set((s) => ({ showDriftHud: !s.showDriftHud })),
  setPrimaryChannel: (primaryChannel) => set({ primaryChannel }),
  /** Switch between Original and a pre-rendered tier. The caller is
   *  responsible for converting the current playback position into
   *  the new mode's time axis *before* calling this — that extra
   *  trip-time/seek choreography lives in the SourceControls UI and
   *  in PlayerShell's seekTripTime helper. This action just swaps
   *  the flags atomically. */
  setSourceMode: (sourceMode, activeSpeedCurve) =>
    // Switching source axis invalidates any gap overlays — the engine
    // rebuilds and re-derives them for the new lineup.
    set({ sourceMode, activeSpeedCurve, gappedChannels: {} }),

  setChannelGapped: (label, gapped) =>
    set((s) => {
      if (!!s.gappedChannels[label] === gapped) return s; // no-op, avoid churn
      return { gappedChannels: { ...s.gappedChannels, [label]: gapped } };
    }),

  refreshTripTags: async (tripId) => {
    const archive = useStore.getState().currentArchive;
    set({ tagsLoadingTripId: tripId });
    try {
      const tags = await getTagsForTrip(tripId);
      if (useStore.getState().currentArchive !== archive) return;
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
      if (useStore.getState().currentArchive === archive) set({ tagsLoadingTripId: null });
    }
  },
  refreshTripTagCounts: async () => {
    const archive = useStore.getState().currentArchive;
    try {
      const counts = await getTagCountsByTrip();
      if (useStore.getState().currentArchive !== archive) return;
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
    const archive = useStore.getState().currentArchive;
    try {
      const places = await listPlaces();
      if (useStore.getState().currentArchive !== archive) return;
      const placesById: Record<number, (typeof places)[number]> = {};
      for (const p of places) placesById[p.id] = p;
      set({ places, placesById });
    } catch (e) {
      console.error("refreshPlaces failed", e);
    }
  },

  startAnalysisScan: async (scanIds, scope, tripIds) => {
    set({
      scanRunning: true,
      scanStartTotal: 0,
      scanProgress: null,
      scanLastResult: null,
    });
    try {
      await ipcStartScan(scanIds, scope, tripIds ?? null);
    } catch (e) {
      console.error("startAnalysisScan failed", e);
      set({ scanRunning: false });
      throw e;
    }
  },
  cancelAnalysisScan: async () => {
    await ipcCancelScan();
  },
  refreshScanCoverage: async () => {
    const archive = useStore.getState().currentArchive;
    try {
      const coverage = await ipcListScanCoverage();
      if (useStore.getState().currentArchive !== archive) return;
      set({ scanCoverage: coverage });
    } catch (e) {
      console.error("refreshScanCoverage failed", e);
    }
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
    const archive = useStore.getState().currentArchive;
    try {
      const jobs = await ipcListTimelapseJobs();
      if (useStore.getState().currentArchive !== archive) return;
      set({ timelapseJobs: jobs });
      // Job completions push timelapse_bytes up and may flip a trip
      // into the reclaimable set — refresh totals together.
      void useStore.getState().refreshLibrarySummary();
      // Rebuilds and merges can create orphan files (old trip_id
      // filenames left over). Keep the Prune badge accurate without
      // requiring the user to visit the Timelapse tab.
      void useStore.getState().refreshOrphanCount();
    } catch (e) {
      console.error("refreshTimelapseJobs failed", e);
    }
  },
  refreshTripGps: async (tripId) => {
    const archive = useStore.getState().currentArchive;
    try {
      const archived = await ipcLoadTripGps(tripId);
      if (useStore.getState().currentArchive !== archive) return;
      // Set unconditionally (even when empty) so a rebuild that produced
      // GPS replaces a stale/empty entry, and a trip with genuinely no
      // GPS falls back correctly to the per-file pool.
      set((s) => ({
        tripGpsByTrip: { ...s.tripGpsByTrip, [tripId]: archived },
      }));
    } catch (e) {
      console.error("refreshTripGps failed", e);
    }
  },
  refreshOrphanCount: async () => {
    const archive = useStore.getState().currentArchive;
    try {
      const n = await ipcCountOrphanTimelapseFiles();
      if (useStore.getState().currentArchive !== archive) return;
      set({ orphanTimelapseCount: n });
    } catch (e) {
      console.error("refreshOrphanCount failed", e);
    }
  },
  refreshLibrarySummary: async () => {
    const archive = useStore.getState().currentArchive;
    try {
      const summary = await ipcGetLibraryStorageSummary();
      if (useStore.getState().currentArchive !== archive) return;
      set({ librarySummary: summary });
    } catch (e) {
      console.error("refreshLibrarySummary failed", e);
    }
  },
  setReclaimableFilter: (reclaimableFilter) => set({ reclaimableFilter }),
  startTimelapseRun: async (args) => {
    // Optimistically flip both flags so the Progress card renders the
    // "Scanning library…" indicator instantly. The backend's
    // `timelapse:scanning` events still gate it authoritatively
    // (false when scan finishes, before the encode loop emits start);
    // this just removes the brief blank-progress flash between the
    // click and the first event landing over IPC.
    set({
      timelapseRunning: true,
      timelapseScanning: true,
      timelapseProgress: null,
      timelapseLastResult: null,
    });
    try {
      await ipcStartTimelapse(args);
    } catch (e) {
      console.error("startTimelapseRun failed", e);
      set({ timelapseRunning: false, timelapseScanning: false });
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
