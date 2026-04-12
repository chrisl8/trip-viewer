import { create } from "zustand";
import type { GpsPoint, ScanError, Trip } from "../types/model";

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
}

export interface AppState extends LibrarySlice, PlaybackSlice {
  status: AppStatus;
  error: string | null;

  setStatus: (s: AppStatus) => void;
  setError: (e: string | null) => void;
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

  status: "idle",
  error: null,

  setStatus: (status) => set({ status }),
  setError: (error) => set({ error, status: error ? "error" : "idle" }),
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
    }),
  setActiveSegmentId: (activeSegmentId) =>
    set({ activeSegmentId, currentTime: 0, isPlaying: false }),
  setCurrentTime: (currentTime) => set({ currentTime }),
  setIsPlaying: (isPlaying) => set({ isPlaying }),
  setSpeed: (speed) => set({ speed }),
  setDrift: (drift) => set({ drift }),
  toggleDriftHud: () => set((s) => ({ showDriftHud: !s.showDriftHud })),
}));
