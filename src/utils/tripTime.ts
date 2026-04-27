/**
 * Trip-time helpers that centralize the math currently inlined in
 * `Timeline.tsx` and `TransportControls.tsx`. Single source of truth
 * so original-vs-tiered playback routing happens in one place.
 *
 * "Trip time" (a.k.a. concat-time) means "seconds from the start of
 * the trip with inter-segment gaps collapsed" — the timeline and map
 * already work in this axis. In Original playback mode the store's
 * `currentTime` is segment-local and we accumulate segment durations
 * up to the active segment to get trip-time. In tiered playback mode
 * the store's `currentTime` is file-time (in the pre-rendered MP4)
 * and we pass it through the speed curve to get trip-time.
 */

import type { Trip } from "../types/model";
import {
  concatToFile,
  fileToConcat,
  totalConcatDuration,
  type CurveSegment,
} from "./speedCurve";

export type SourceMode = "original" | "8x" | "16x" | "60x";

/**
 * Compute trip-time (concat-time) given the current playback state.
 *
 * - Original mode: walks segments until `activeSegmentId`, adds
 *   segment-local `currentTime`. Falls back to the first segment if
 *   no active segment is set.
 * - Tiered mode: runs `currentTime` (file-time) through the curve.
 *   If `curve` is missing, falls back to linear file × tier rate
 *   using the first segment of the curve — practically impossible
 *   because callers ensure a curve exists in tiered mode, but guards
 *   against NaN/undefined.
 */
export function computeTripTime(
  trip: Trip | undefined,
  activeSegmentId: string | null,
  currentTime: number,
  sourceMode: SourceMode,
  curve: CurveSegment[] | null,
): number {
  if (!trip) return 0;

  if (sourceMode === "original") {
    const activeId = activeSegmentId ?? trip.segments[0]?.id;
    let cumulative = 0;
    for (const seg of trip.segments) {
      if (seg.id === activeId) return cumulative + currentTime;
      cumulative += seg.durationS;
    }
    return cumulative + currentTime;
  }

  // Tiered: currentTime is file-time; route through the curve.
  if (!curve || curve.length === 0) return 0;
  return fileToConcat(currentTime, curve);
}

/**
 * Result of a trip-time seek request. The shape differs between
 * modes because the two video stacks seek differently: Original
 * walks segments (cross-segment seek requires setActiveSegmentId +
 * a pending seek); tiered seeks a single file directly.
 */
export type SeekTarget =
  | {
      mode: "original";
      /** Segment to load (may be the currently-active one). */
      activeSegmentId: string;
      /** Seconds within that segment to seek to. */
      segmentLocalTime: number;
    }
  | {
      mode: "tiered";
      /** File-time position in the tiered MP4. */
      fileTime: number;
      /** The segment whose concat range contains this moment — used
       *  to keep `activeSegmentId` in sync for SegmentTagBar, even
       *  though the video element itself doesn't switch sources. */
      virtualActiveSegmentId: string | null;
    };

/**
 * Convert a trip-time target into a seek that the player layer can
 * execute. Clamps the input to `[0, trip-total]`.
 */
export function seekTripTime(
  tripTime: number,
  trip: Trip | undefined,
  sourceMode: SourceMode,
  curve: CurveSegment[] | null,
): SeekTarget | null {
  if (!trip || trip.segments.length === 0) return null;

  const total = trip.segments.reduce((sum, s) => sum + s.durationS, 0);
  const clamped = Math.max(0, Math.min(total, tripTime));

  if (sourceMode === "original") {
    let cumulative = 0;
    for (const seg of trip.segments) {
      const next = cumulative + seg.durationS;
      if (clamped < next || seg === trip.segments[trip.segments.length - 1]) {
        return {
          mode: "original",
          activeSegmentId: seg.id,
          segmentLocalTime: Math.max(0, clamped - cumulative),
        };
      }
      cumulative = next;
    }
    // Unreachable because the loop returns on the last segment, but TS
    // needs a value.
    return null;
  }

  if (!curve || curve.length === 0) return null;
  return {
    mode: "tiered",
    fileTime: concatToFile(clamped, curve),
    virtualActiveSegmentId: activeSegmentAtConcatTime(trip, clamped),
  };
}

/**
 * Given a trip and a concat-time, return the ID of the segment whose
 * cumulative-duration range contains that moment. Returns null for an
 * empty trip. Used in tiered mode to keep `activeSegmentId` honest so
 * the tag bar and timeline highlights follow along.
 */
export function activeSegmentAtConcatTime(
  trip: Trip | undefined,
  concatTime: number,
): string | null {
  if (!trip || trip.segments.length === 0) return null;
  let cumulative = 0;
  for (const seg of trip.segments) {
    const next = cumulative + seg.durationS;
    if (concatTime < next) return seg.id;
    cumulative = next;
  }
  return trip.segments[trip.segments.length - 1].id;
}

/**
 * Trip-total duration in trip-time seconds, mode-agnostic.
 * Matches the existing inline reduce in Timeline.tsx:46.
 */
export function tripTotalDuration(trip: Trip | undefined): number {
  if (!trip) return 0;
  return trip.segments.reduce((sum, s) => sum + s.durationS, 0);
}

/**
 * Cross-check helper: what's the trip-total in concat-time according
 * to the curve, for sanity-check assertions in dev. Should equal
 * `tripTotalDuration` within rounding error for a well-formed curve.
 */
export function tripTotalFromCurve(curve: CurveSegment[]): number {
  return totalConcatDuration(curve);
}
