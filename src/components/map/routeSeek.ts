import type { GpsPoint } from "../../types/model";

export type ScreenPoint = { x: number; y: number };
export function validRoutePoint(p: GpsPoint): boolean {
  return p.fixOk && Number.isFinite(p.tOffsetS) && p.tOffsetS >= 0 &&
    Number.isFinite(p.lat) && Math.abs(p.lat) <= 90 &&
    Number.isFinite(p.lon) && Math.abs(p.lon) <= 180;
}
export function connectedRoutePoints(a: GpsPoint, b: GpsPoint): boolean {
  // Match the GPS interpolator: never invent a position across a missing fix.
  const dt = b.tOffsetS - a.tOffsetS;
  return validRoutePoint(a) && validRoutePoint(b) && dt > 0 && dt <= 2 && Math.abs(a.lon - b.lon) <= 180;
}
export function routeRuns(points: GpsPoint[]): GpsPoint[][] {
  const runs: GpsPoint[][] = [];
  let run: GpsPoint[] = [];
  for (const p of points) {
    if (!validRoutePoint(p)) { run = []; continue; }
    if (!run.length || !connectedRoutePoints(run[run.length - 1], p)) {
      run = []; runs.push(run);
    }
    run.push(p);
  }
  return runs;
}

/** Project onto recorded route edges in screen space at the current map zoom.
 * Repeated locations/crossings prefer the time nearest the current playhead.
 * Returns trip time; the existing timeline seek path handles source mapping. */
export function routeSeekTime(
  points: GpsPoint[], click: ScreenPoint, project: (p: GpsPoint) => ScreenPoint,
  currentTime: number, maxDistancePx = 12,
): number | null {
  let bestDistance = maxDistancePx ** 2;
  let bestTime: number | null = null;
  const consider = (p: ScreenPoint, time: number) => {
    const distance = (p.x - click.x) ** 2 + (p.y - click.y) ** 2;
    if (!Number.isFinite(distance)) return;
    if (distance < bestDistance - 1e-6 ||
        (Math.abs(distance - bestDistance) <= 1e-6 &&
         (bestTime === null || Math.abs(time - currentTime) < Math.abs(bestTime - currentTime)))) {
      bestDistance = distance; bestTime = time;
    }
  };
  for (let i = 0; i < points.length; i++) {
    const b = points[i];
    if (!validRoutePoint(b)) continue;
    const pb = project(b);
    consider(pb, b.tOffsetS);
    const a = points[i - 1];
    if (!a || !connectedRoutePoints(a, b)) continue;
    const pa = project(a);
    const dx = pb.x - pa.x, dy = pb.y - pa.y;
    const length2 = dx * dx + dy * dy;
    if (length2 === 0) continue; // stationary points are handled above
    const fraction = Math.max(0, Math.min(1, ((click.x - pa.x) * dx + (click.y - pa.y) * dy) / length2));
    consider({ x: pa.x + fraction * dx, y: pa.y + fraction * dy }, a.tOffsetS + fraction * (b.tOffsetS - a.tOffsetS));
  }
  return bestTime;
}
