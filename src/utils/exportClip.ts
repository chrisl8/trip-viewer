import type { ExportAudioArg, ExportSegmentArg } from "../ipc/export";
import type { Trip } from "../types/model";

/** Canonical channel order, mirroring `label_rank` in the Rust model. */
const CANONICAL = ["Front", "Interior", "Rear"];

export function compareLabels(a: string, b: string): number {
  const ra = CANONICAL.indexOf(a);
  const rb = CANONICAL.indexOf(b);
  const pa = ra === -1 ? 10 : ra;
  const pb = rb === -1 ? 10 : rb;
  if (pa !== pb) return pa - pb;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Every channel label that appears in the trip, in canonical order. */
export function tripChannelLabels(trip: Trip): string[] {
  const seen = new Set<string>();
  for (const seg of trip.segments) {
    for (const ch of seg.channels) seen.add(ch.label);
  }
  return [...seen].sort(compareLabels);
}

export function tripSegmentsForExport(trip: Trip): ExportSegmentArg[] {
  return trip.segments.map((seg) => ({
    durationS: seg.durationS,
    files: seg.channels.map((ch) => ({ label: ch.label, path: ch.filePath })),
  }));
}

export function tripDurationS(trip: Trip): number {
  return trip.segments.reduce((sum, seg) => sum + seg.durationS, 0);
}

/**
 * Parse a time typed by the user: plain seconds ("90", "12.5") or a
 * clock ("1:30", "1:02:03", "0:07.5"). Returns null when it isn't one.
 */
export function parseClockInput(text: string): number | null {
  const t = text.trim();
  if (t === "") return null;
  const parts = t.split(":");
  if (parts.length > 3) return null;
  let total = 0;
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    const isLast = i === parts.length - 1;
    const ok = isLast ? /^\d+(\.\d+)?$/.test(p) : /^\d+$/.test(p);
    if (!ok) return null;
    total = total * 60 + Number(p);
  }
  return Number.isFinite(total) ? total : null;
}

/** `m:ss`, or `h:mm:ss` from one hour up. */
export function formatClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const ss = String(sec).padStart(2, "0");
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${ss}`;
  return `${m}:${ss}`;
}

/**
 * Suggested file name from the wall-clock moment the clip starts at, e.g.
 * `TripViewer_20261023-142152.mp4`. Done with plain arithmetic on the
 * trip's own timestamp so the result doesn't depend on the viewer's
 * time zone.
 */
export function defaultClipFileName(trip: Trip, startS: number): string {
  const m = /(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2}):(\d{2})/.exec(
    trip.startTime,
  );
  if (!m) return "TripViewer_clip.mp4";
  const base = Date.UTC(
    Number(m[1]),
    Number(m[2]) - 1,
    Number(m[3]),
    Number(m[4]),
    Number(m[5]),
    Number(m[6]),
  );
  const d = new Date(base + Math.floor(startS) * 1000);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return (
    `TripViewer_${p(d.getUTCFullYear(), 4)}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}` +
    `-${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}.mp4`
  );
}

/** Validate a start/length pair against the trip. Null = not exportable. */
export function validateRange(
  startS: number | null,
  lengthS: number | null,
  totalS: number,
): { startS: number; durationS: number } | null {
  if (startS == null || lengthS == null) return null;
  if (!(startS >= 0) || startS >= totalS) return null;
  if (!(lengthS >= 1)) return null;
  return { startS, durationS: Math.min(lengthS, totalS - startS) };
}

/**
 * Volumes are entered as percentages per channel; every channel above 0%
 * becomes an audio source. Several sources are mixed, none = silent.
 */
export function audioArgsFromPercent(
  percentByLabel: Record<string, number>,
): ExportAudioArg[] {
  return Object.keys(percentByLabel)
    .filter((l) => percentByLabel[l] > 0)
    .sort(compareLabels)
    .map((label) => ({ label, volume: percentByLabel[label] / 100 }));
}
