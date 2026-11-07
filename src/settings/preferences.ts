import { useSyncExternalStore } from "react";

export type SpeedUnit = "kmh" | "mph";
export type CameraModelPreference = "auto" | "viofoA229Pro";

export interface UserPreferences {
  speedUnit: SpeedUnit;
  cameraModel: CameraModelPreference;
  showMap: boolean;
}

const STORAGE_KEY = "tripviewer.userPreferences.v1";

export const DEFAULT_PREFERENCES: UserPreferences = {
  speedUnit: "kmh",
  cameraModel: "auto",
  showMap: true,
};

function normalize(value: Partial<UserPreferences> | null | undefined): UserPreferences {
  return {
    speedUnit: value?.speedUnit === "mph" ? "mph" : "kmh",
    cameraModel:
      value?.cameraModel === "viofoA229Pro" ? "viofoA229Pro" : "auto",
    showMap: value?.showMap !== false,
  };
}

function loadPreferences(): UserPreferences {
  if (typeof window === "undefined") return DEFAULT_PREFERENCES;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_PREFERENCES;
    return normalize(JSON.parse(raw) as Partial<UserPreferences>);
  } catch (e) {
    console.warn("[settings] failed to load preferences; using defaults", e);
    return DEFAULT_PREFERENCES;
  }
}

let current = loadPreferences();
const listeners = new Set<() => void>();

function applyDocumentPreferences(preferences: UserPreferences) {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.tripviewerMap = preferences.showMap
    ? "on"
    : "off";
}

applyDocumentPreferences(current);

export function getPreferencesSnapshot(): UserPreferences {
  return current;
}

export function savePreferences(preferences: UserPreferences): void {
  current = normalize(preferences);
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch (e) {
    console.warn("[settings] failed to persist preferences", e);
  }
  applyDocumentPreferences(current);
  for (const listener of listeners) listener();
}

export function subscribePreferences(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function usePreferences(): UserPreferences {
  return useSyncExternalStore(
    subscribePreferences,
    getPreferencesSnapshot,
    () => DEFAULT_PREFERENCES,
  );
}
