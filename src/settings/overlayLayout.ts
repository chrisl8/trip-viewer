import { LABEL_FRONT, LABEL_INTERIOR, LABEL_REAR } from "../types/model";

export type CameraLayout = "grid" | "overlay";
export type OverlayFrame = "rounded" | "mirror";
export interface OverlayCrop { zoom: number; x: number; y: number }
export interface OverlayPanel {
  // Fractions of the video stage, independent of screen resolution/fullscreen.
  x: number; y: number; width: number; height: number;
  frame: OverlayFrame;
  order: number;
  // x/y range from -1 to 1 across the available overscan, never exposing gaps.
  crop: OverlayCrop;
}
export type OverlayPanels = Record<string, OverlayPanel>;

function finite(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}
export function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export function defaultOverlayPanel(label: string, index = 0): OverlayPanel {
  return {
    x: label === LABEL_REAR ? .28 : label === LABEL_INTERIOR ? .72 : .04 + (index % 4) * .06,
    y: label === LABEL_REAR ? .04 : label === LABEL_INTERIOR ? .65 : .54 + (index % 4) * .06,
    width: label === LABEL_REAR ? .44 : .25,
    height: label === LABEL_REAR ? .22 : .3,
    frame: label === LABEL_REAR ? "mirror" : "rounded",
    order: label === LABEL_REAR ? 2 : Math.min(49, index + 3),
    crop: { zoom: 1, x: 0, y: 0 },
  };
}

export function normalizeOverlayPanel(value: unknown, label: string, index = 0): OverlayPanel {
  const defaults = defaultOverlayPanel(label, index);
  const panel = (value && typeof value === "object" ? value : {}) as Partial<OverlayPanel>;
  const width = clamp(finite(panel.width, defaults.width), .12, 1);
  const height = clamp(finite(panel.height, defaults.height), .1, 1);
  return {
    x: clamp(finite(panel.x, defaults.x), 0, 1 - width),
    y: clamp(finite(panel.y, defaults.y), 0, 1 - height),
    width, height,
    frame: panel.frame === "mirror" || panel.frame === "rounded" ? panel.frame : defaults.frame,
    order: Math.round(clamp(finite(panel.order, defaults.order), 1, 49)),
    crop: {
      zoom: clamp(finite(panel.crop?.zoom, 1), 1, 8),
      x: clamp(finite(panel.crop?.x, 0), -1, 1),
      y: clamp(finite(panel.crop?.y, 0), -1, 1),
    },
  };
}

export function normalizeOverlayPanels(value: unknown): OverlayPanels {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).map(([label, panel], index) =>
    [label, normalizeOverlayPanel(panel, label, index)]));
}

export function overlayPanelFor(panels: OverlayPanels, label: string, index = 0): OverlayPanel {
  return normalizeOverlayPanel(Object.prototype.hasOwnProperty.call(panels, label) ? panels[label] : undefined, label, index);
}

export function overlayBackground(labels: string[]): string | null {
  return labels.includes(LABEL_FRONT) ? LABEL_FRONT : labels[0] ?? null;
}

export function moveOverlay(panel: OverlayPanel, dx: number, dy: number): OverlayPanel {
  return { ...panel, x: clamp(panel.x + dx, 0, 1 - panel.width), y: clamp(panel.y + dy, 0, 1 - panel.height) };
}

export function resizeOverlay(panel: OverlayPanel, dx: number, dy: number): OverlayPanel {
  return { ...panel, width: clamp(panel.width + dx, .12, 1 - panel.x), height: clamp(panel.height + dy, .1, 1 - panel.y) };
}

export function cropGeometry(crop: OverlayCrop, width: number, height: number, mediaWidth: number, mediaHeight: number) {
  if (![width, height, mediaWidth, mediaHeight].every(v => Number.isFinite(v) && v > 0)) return null;
  const scale = Math.max(width / mediaWidth, height / mediaHeight) * crop.zoom;
  const videoWidth = mediaWidth * scale;
  const videoHeight = mediaHeight * scale;
  const maxX = Math.max(0, (videoWidth - width) / 2);
  const maxY = Math.max(0, (videoHeight - height) / 2);
  return { width: videoWidth, height: videoHeight, maxX, maxY,
    left: -maxX + crop.x * maxX, top: -maxY + crop.y * maxY };
}

export function panOverlayCrop(crop: OverlayCrop, dx: number, dy: number, width: number, height: number, mediaWidth: number, mediaHeight: number): OverlayCrop {
  const geometry = cropGeometry(crop, width, height, mediaWidth, mediaHeight);
  if (!geometry) return crop;
  return { ...crop,
    x: geometry.maxX > 0 ? clamp(crop.x + dx / geometry.maxX, -1, 1) : crop.x,
    y: geometry.maxY > 0 ? clamp(crop.y + dy / geometry.maxY, -1, 1) : crop.y };
}

export function bringOverlayToFront(panels: OverlayPanels, labels: string[], label: string): OverlayPanels {
  const current = Object.fromEntries(labels.map((item, index) => [item, overlayPanelFor(panels, item, index)]));
  const ordered = labels.filter(item => item !== label)
    .sort((a, b) => current[a].order - current[b].order);
  ordered.push(label);
  return { ...panels, ...Object.fromEntries(ordered.map((item, index) =>
    [item, { ...current[item], order: index + 1 }])) };
}
