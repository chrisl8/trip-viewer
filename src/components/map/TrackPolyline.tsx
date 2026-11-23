import { useMemo } from "react";
import { Polyline } from "react-leaflet";
import type { GpsPoint } from "../../types/model";
import { routeRuns } from "./routeContinuity";

interface Props {
  points: GpsPoint[];
}

export function TrackPolyline({ points }: Props) {
  const runs = useMemo(() => routeRuns(points).filter(run => run.length >= 2), [points]);

  if (runs.length === 0) return null;

  return (
    <>{runs.map((run, index) => <Polyline key={index}
      positions={run.map(p => [p.lat, p.lon] as [number, number])}
      pathOptions={{ color: "#3b82f6", weight: 3, opacity: 0.7 }}
    />)}</>
  );
}
