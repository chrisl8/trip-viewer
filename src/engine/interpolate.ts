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
