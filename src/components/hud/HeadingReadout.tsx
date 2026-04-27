import { useMemo } from "react";
import {
  HEADING_HOLD_THRESHOLD_MPS,
  interpolateGps,
  lastMovingHeading,
} from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";

interface Props {
  gpsPoints: GpsPoint[];
  /** Time to interpolate at — segment-local in Original mode, concat-time in tiered. */
  interpolationTime: number;
  activeSegment: Segment | null;
}

const COMPASS: [number, string][] = [
  [0, "N"], [45, "NE"], [90, "E"], [135, "SE"],
  [180, "S"], [225, "SW"], [270, "W"], [315, "NW"], [360, "N"],
];

function degreesToCompass(deg: number): string {
  const norm = ((deg % 360) + 360) % 360;
  for (let i = 0; i < COMPASS.length - 1; i++) {
    const mid = (COMPASS[i][0] + COMPASS[i + 1][0]) / 2;
    if (norm < mid) return COMPASS[i][1];
  }
  return "N";
}

export function HeadingReadout({
  gpsPoints,
  interpolationTime,
  activeSegment,
}: Props) {
  const interp = useMemo(
    () =>
      activeSegment ? interpolateGps(gpsPoints, interpolationTime) : null,
    [gpsPoints, interpolationTime, activeSegment],
  );

  // GPS-derived heading is unreliable below the moving threshold —
  // it jitters across the dial as the receiver chases position noise.
  // Hold the last trustworthy heading while stopped or creeping so the
  // readout doesn't flicker. Falls back to the live (jittery) value
  // only if the vehicle hasn't yet moved at all on this trip.
  const displayHeading = useMemo(() => {
    if (!interp) return null;
    if (interp.speedMps >= HEADING_HOLD_THRESHOLD_MPS) return interp.headingDeg;
    return lastMovingHeading(gpsPoints, interpolationTime) ?? interp.headingDeg;
  }, [interp, gpsPoints, interpolationTime]);

  if (!interp || displayHeading === null) return null;

  return (
    <div
      className={`min-w-[4.25rem] rounded-md bg-black/70 px-3 py-2 text-center backdrop-blur ${interp.stale ? "opacity-40" : ""}`}
    >
      <div className="text-2xl font-bold text-white">
        {degreesToCompass(displayHeading)}
      </div>
      <div className="text-[10px] font-medium uppercase tabular-nums tracking-wider text-neutral-400">
        {Math.round(displayHeading)}&deg;
      </div>
    </div>
  );
}
