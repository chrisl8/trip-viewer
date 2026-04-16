import { useMemo } from "react";
import { useStore } from "../../state/store";
import { interpolateGps } from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";

interface Props {
  gpsPoints: GpsPoint[];
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

export function HeadingReadout({ gpsPoints, activeSegment }: Props) {
  const currentTime = useStore((s) => s.currentTime);

  const interp = useMemo(
    () => (activeSegment ? interpolateGps(gpsPoints, currentTime) : null),
    [gpsPoints, currentTime, activeSegment],
  );

  if (!interp) return null;

  return (
    <div
      className={`min-w-[4.25rem] rounded-md bg-black/70 px-3 py-2 text-center backdrop-blur ${interp.stale ? "opacity-40" : ""}`}
    >
      <div className="text-2xl font-bold text-white">
        {degreesToCompass(interp.headingDeg)}
      </div>
      <div className="text-[10px] font-medium uppercase tabular-nums tracking-wider text-neutral-400">
        {Math.round(interp.headingDeg)}&deg;
      </div>
    </div>
  );
}
