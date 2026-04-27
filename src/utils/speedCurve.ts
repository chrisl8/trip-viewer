/**
 * Speed-curve mapper: translates file-time (what the currently-playing
 * MP4 reports via `<video>.currentTime`) ↔ concat-time (trip-time,
 * seconds-from-trip-start with segment gaps collapsed).
 *
 * The curve is produced at encode time by the Rust
 * `speed_curve::build_curve` and persisted as JSON on the
 * `timelapse_jobs.speed_curve_json` column. The shape here matches the
 * backend's `CurveSegment` serialization exactly (camelCase).
 *
 * Curves are always non-empty and cover [0, totalConcatDuration]
 * contiguously. Each segment has a constant `rate` (concat-seconds per
 * file-second). For tier 8x: one segment at rate=8. For variable tiers:
 * alternating base and event rates around GPS-detected event windows.
 */

export interface CurveSegment {
  /** Concat-time start (seconds from trip start). */
  concatStart: number;
  /** Concat-time end (exclusive in practice; inclusive at trip end). */
  concatEnd: number;
  /** Concat-seconds per file-second. rate=8 means this segment
   *  compresses 8 seconds of trip time into 1 second of MP4. */
  rate: number;
}

/** Trip-total duration in concat-time for a curve. */
export function totalConcatDuration(curve: CurveSegment[]): number {
  if (curve.length === 0) return 0;
  return curve[curve.length - 1].concatEnd;
}

/** Trip-total duration in file-time for a curve (length of the MP4). */
export function totalFileDuration(curve: CurveSegment[]): number {
  let t = 0;
  for (const seg of curve) {
    t += (seg.concatEnd - seg.concatStart) / seg.rate;
  }
  return t;
}

/**
 * Map a file-time (what the video element reports) to concat-time
 * (trip-time). Clamped to [0, totalConcat] for out-of-range inputs.
 */
export function fileToConcat(
  fileTime: number,
  curve: CurveSegment[],
): number {
  if (curve.length === 0) return 0;
  if (fileTime <= 0) return curve[0].concatStart;

  let cumulativeFile = 0;
  for (const seg of curve) {
    const segFileSpan = (seg.concatEnd - seg.concatStart) / seg.rate;
    if (fileTime <= cumulativeFile + segFileSpan) {
      const offset = fileTime - cumulativeFile;
      return seg.concatStart + offset * seg.rate;
    }
    cumulativeFile += segFileSpan;
  }
  // Past the end — clamp to trip total.
  return curve[curve.length - 1].concatEnd;
}

/**
 * Map a concat-time (trip-time) to file-time (position in the MP4).
 * Clamped to [0, totalFile] for out-of-range inputs.
 */
export function concatToFile(
  concatTime: number,
  curve: CurveSegment[],
): number {
  if (curve.length === 0) return 0;
  if (concatTime <= curve[0].concatStart) return 0;

  let cumulativeFile = 0;
  for (const seg of curve) {
    if (concatTime <= seg.concatEnd) {
      const offset = concatTime - seg.concatStart;
      return cumulativeFile + offset / seg.rate;
    }
    cumulativeFile += (seg.concatEnd - seg.concatStart) / seg.rate;
  }
  // Past the trip end — clamp to file total.
  return cumulativeFile;
}

/**
 * Convenience: a single-segment curve at a constant rate covering
 * `totalDurationS` of concat-time. Used for the fallback when a
 * timelapse_jobs row has no persisted curve JSON (legacy data), and
 * as a trivial identity curve (rate=1) when needed.
 */
export function linearCurve(
  totalDurationS: number,
  rate: number,
): CurveSegment[] {
  return [
    {
      concatStart: 0,
      concatEnd: Math.max(0, totalDurationS),
      rate: Math.max(1, rate),
    },
  ];
}

/**
 * Parse a JSON string (as persisted on `timelapse_jobs.speed_curve_json`)
 * into a curve, or return null if parsing fails or the shape is wrong.
 * Callers fall back to `linearCurve` using the tier's base rate.
 */
export function parseCurveJson(json: string | null | undefined): CurveSegment[] | null {
  if (!json) return null;
  try {
    const parsed = JSON.parse(json);
    if (!Array.isArray(parsed) || parsed.length === 0) return null;
    const valid = parsed.every(
      (s) =>
        typeof s === "object" &&
        s !== null &&
        typeof s.concatStart === "number" &&
        typeof s.concatEnd === "number" &&
        typeof s.rate === "number" &&
        s.rate > 0 &&
        s.concatEnd >= s.concatStart,
    );
    return valid ? (parsed as CurveSegment[]) : null;
  } catch {
    return null;
  }
}
