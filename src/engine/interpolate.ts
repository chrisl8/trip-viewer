import type { GpsPoint } from "../types/model";

export interface InterpolatedGps {
  lat: number;
  lon: number;
  speedMps: number;
  headingDeg: number;
  altitudeM: number;
  stale: boolean;
}

export function interpolateGps(
  points: GpsPoint[],
  tOffsetS: number,
): InterpolatedGps | null {
  if (points.length === 0) return null;

  if (tOffsetS <= points[0].tOffsetS) {
    const p = points[0];
    return {
      lat: p.lat,
      lon: p.lon,
      speedMps: p.speedMps,
      headingDeg: p.headingDeg,
      altitudeM: p.altitudeM,
      stale: tOffsetS < points[0].tOffsetS - 1,
    };
  }

  const last = points[points.length - 1];
  if (tOffsetS >= last.tOffsetS) {
    return {
      lat: last.lat,
      lon: last.lon,
      speedMps: last.speedMps,
      headingDeg: last.headingDeg,
      altitudeM: last.altitudeM,
      stale: tOffsetS > last.tOffsetS + 2,
    };
  }

  // Binary search for bracketing pair
  let lo = 0;
  let hi = points.length - 1;
  while (lo < hi - 1) {
    const mid = (lo + hi) >> 1;
    if (points[mid].tOffsetS <= tOffsetS) lo = mid;
    else hi = mid;
  }

  const a = points[lo];
  const b = points[hi];
  const gap = b.tOffsetS - a.tOffsetS;

  // Freeze on GPS gaps > 2s
  if (gap > 2) {
    return {
      lat: a.lat,
      lon: a.lon,
      speedMps: a.speedMps,
      headingDeg: a.headingDeg,
      altitudeM: a.altitudeM,
      stale: true,
    };
  }

  const alpha = gap > 0 ? (tOffsetS - a.tOffsetS) / gap : 0;

  return {
    lat: a.lat + (b.lat - a.lat) * alpha,
    lon: a.lon + (b.lon - a.lon) * alpha,
    speedMps: a.speedMps + (b.speedMps - a.speedMps) * alpha,
    headingDeg: lerpAngle(a.headingDeg, b.headingDeg, alpha),
    altitudeM: a.altitudeM + (b.altitudeM - a.altitudeM) * alpha,
    stale: false,
  };
}

function lerpAngle(a: number, b: number, t: number): number {
  let diff = ((b - a + 540) % 360) - 180;
  return ((a + diff * t) % 360 + 360) % 360;
}

/** Below this, GPS heading is dominated by receiver noise rather than
 *  vehicle motion — it can drift by tens of degrees per second with no
 *  real change in orientation. Matches the moving-filter threshold in
 *  `src-tauri/src/timelapse/events.rs::MOVING_MPS`. */
export const HEADING_HOLD_THRESHOLD_MPS = 2.0;

/** Below this, the receiver still reports small non-zero speeds from
 *  sub-meter position drift — enough to flicker the rounded mph readout
 *  between 0 and 1 at a full stop. Snap the displayed speed to zero
 *  below this floor. Matches `LONG_STOP_MPS` in the events detector. */
export const STOPPED_DISPLAY_THRESHOLD_MPS = 1.0;

/**
 * Heading from the most recent GPS sample at-or-before `tOffsetS`
 * whose speed was at or above `HEADING_HOLD_THRESHOLD_MPS`. Used to
 * "freeze" the displayed compass on the last trustworthy heading
 * while the vehicle is stopped or creeping, so the readout doesn't
 * flicker through GPS heading noise at zero speed.
 *
 * Returns `null` if the trip is empty or the vehicle has not yet
 * exceeded the threshold by `tOffsetS`.
 */
export function lastMovingHeading(
  points: GpsPoint[],
  tOffsetS: number,
): number | null {
  if (points.length === 0) return null;
  if (tOffsetS < points[0].tOffsetS) return null;

  // Binary search for the latest index at-or-before tOffsetS.
  let lo = 0;
  let hi = points.length - 1;
  if (tOffsetS >= points[hi].tOffsetS) {
    lo = hi;
  } else {
    while (lo < hi - 1) {
      const mid = (lo + hi) >> 1;
      if (points[mid].tOffsetS <= tOffsetS) lo = mid;
      else hi = mid;
    }
  }

  for (let i = lo; i >= 0; i--) {
    if (points[i].speedMps >= HEADING_HOLD_THRESHOLD_MPS) {
      return points[i].headingDeg;
    }
  }
  return null;
}
