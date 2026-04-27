import { useMemo } from "react";
import {
  STOPPED_DISPLAY_THRESHOLD_MPS,
  interpolateGps,
} from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";

interface Props {
  gpsPoints: GpsPoint[];
  /** Time to interpolate at — segment-local in Original mode, concat-time in tiered. */
  interpolationTime: number;
  activeSegment: Segment | null;
}

export function SpeedReadout({ gpsPoints, interpolationTime, activeSegment }: Props) {
  const interp = useMemo(
    () =>
      activeSegment ? interpolateGps(gpsPoints, interpolationTime) : null,
    [gpsPoints, interpolationTime, activeSegment],
  );

  if (!interp) return null;

  // Snap to a clean 0 below the stopped threshold so position-noise
  // doesn't flicker the rounded readout between 0 and 1 mph at a stop.
  const effectiveMps =
    interp.speedMps < STOPPED_DISPLAY_THRESHOLD_MPS ? 0 : interp.speedMps;
  const mph = effectiveMps * 2.23694;

  return (
    <div
      className={`min-w-[3.75rem] rounded-md bg-black/70 px-3 py-2 text-center backdrop-blur ${interp.stale ? "opacity-40" : ""}`}
    >
      <div className="text-2xl font-bold tabular-nums text-white">
        {Math.round(mph)}
      </div>
      <div className="text-[10px] font-medium uppercase tracking-wider text-neutral-400">
        mph
      </div>
    </div>
  );
}
