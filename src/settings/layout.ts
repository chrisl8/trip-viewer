import { normalizeOverlayPanels, type CameraLayout, type OverlayPanels } from "./overlayLayout";

export interface LayoutPreferences {
  hiddenChannels: string[];
  sidebarCollapsed: boolean;
  dashboardTimelineCollapsed: boolean;
  primaryShare: number;
  secondarySplit: number;
  mapShare: number;
  timelineHeightPx: number | null;
  cameraLayout: CameraLayout;
  overlayPanels: OverlayPanels;
}

const STORAGE_KEY = "tripviewer.layout.v1";
const listeners = new Set<() => void>();

export function subscribeLayoutPreferences(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export const DEFAULT_LAYOUT_PREFERENCES: LayoutPreferences = {
  hiddenChannels: [],
  sidebarCollapsed: false,
  dashboardTimelineCollapsed: false,
  primaryShare: 2 / 3,
  secondarySplit: 0.5,
  mapShare: 0.25,
  timelineHeightPx: null,
  cameraLayout: "grid",
  overlayPanels: {},
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function finiteOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function normalize(
  value: Partial<LayoutPreferences> | null | undefined,
): LayoutPreferences {
  const timeline = value?.timelineHeightPx;
  return {
    cameraLayout: value?.cameraLayout === "overlay" ? "overlay" : "grid",
    overlayPanels: normalizeOverlayPanels(value?.overlayPanels),
    hiddenChannels: Array.isArray(value?.hiddenChannels) ? [...new Set(value.hiddenChannels.filter((v): v is string => typeof v === "string"))] : [],
    sidebarCollapsed: value?.sidebarCollapsed === true,
    dashboardTimelineCollapsed: value?.dashboardTimelineCollapsed === true,
    primaryShare: clamp(
      finiteOr(value?.primaryShare, DEFAULT_LAYOUT_PREFERENCES.primaryShare),
      0.35,
      0.85,
    ),
    secondarySplit: clamp(
      finiteOr(value?.secondarySplit, DEFAULT_LAYOUT_PREFERENCES.secondarySplit),
      0.2,
      0.8,
    ),
    mapShare: clamp(
      finiteOr(value?.mapShare, DEFAULT_LAYOUT_PREFERENCES.mapShare),
      0.15,
      0.5,
    ),
    timelineHeightPx:
      typeof timeline === "number" && Number.isFinite(timeline)
        ? clamp(timeline, 56, 1000)
        : null,
  };
}

export function getLayoutPreferences(): LayoutPreferences {
  if (typeof window === "undefined") return DEFAULT_LAYOUT_PREFERENCES;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_LAYOUT_PREFERENCES;
    return normalize(JSON.parse(raw) as Partial<LayoutPreferences>);
  } catch (e) {
    console.warn("[layout] failed to load layout preferences; using defaults", e);
    return DEFAULT_LAYOUT_PREFERENCES;
  }
}

export function saveLayoutPreferences(
  patch: Partial<LayoutPreferences>,
): LayoutPreferences {
  const next = normalize({ ...getLayoutPreferences(), ...patch });
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch (e) {
    console.warn("[layout] failed to persist layout preferences", e);
  }
  for (const listener of listeners) listener();
  return next;
}
