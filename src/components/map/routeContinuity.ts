import type { GpsPoint } from "../../types/model";

export function validRoutePoint(p: GpsPoint): boolean {
  return (
    p.fixOk &&
    Number.isFinite(p.tOffsetS) &&
    p.tOffsetS >= 0 &&
    Number.isFinite(p.lat) &&
    Math.abs(p.lat) <= 90 &&
    Number.isFinite(p.lon) &&
    Math.abs(p.lon) <= 180
  );
}
export function connectedRoutePoints(a: GpsPoint, b: GpsPoint): boolean {
  // Match the GPS interpolator: never invent a position across a missing fix.
  const dt = b.tOffsetS - a.tOffsetS;
  return (
    validRoutePoint(a) &&
    validRoutePoint(b) &&
    dt > 0 &&
    dt <= 2 &&
    Math.abs(a.lon - b.lon) <= 180
  );
}
export function routeRuns(points: GpsPoint[]): GpsPoint[][] {
  const runs: GpsPoint[][] = [];
  let run: GpsPoint[] = [];
  for (const p of points) {
    if (!validRoutePoint(p)) {
      run = [];
      continue;
    }
    if (!run.length || !connectedRoutePoints(run[run.length - 1], p)) {
      run = [];
      runs.push(run);
    }
    run.push(p);
  }
  return runs;
}
