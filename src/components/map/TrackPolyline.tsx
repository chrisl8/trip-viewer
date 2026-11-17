import { useMemo } from "react";
import { CircleMarker, Polyline, useMap } from "react-leaflet";
import type { LeafletMouseEvent } from "leaflet";
import type { GpsPoint } from "../../types/model";
import { routeRuns, routeSeekTime } from "./routeSeek";

interface Props {
  points: GpsPoint[];
  currentTripTime: number;
  onSeekTripTime: (time: number) => void;
}
export function TrackPolyline({ points, currentTripTime, onSeekTripTime }: Props) {
  const map = useMap();
  const runs = useMemo(() => routeRuns(points).map(run => ({
    points: run, positions: run.map(p => [p.lat, p.lon] as [number, number]),
  })), [points]);
  const click = (event: LeafletMouseEvent) => {
    const time = routeSeekTime(points, map.latLngToLayerPoint(event.latlng),
      p => map.latLngToLayerPoint([p.lat, p.lon]), currentTripTime);
    if (time !== null) onSeekTripTime(time);
  };
  return <>{runs.map(({ points: run, positions }, index) => run.length === 1 ? (
    <CircleMarker key={index} center={[run[0].lat, run[0].lon]} radius={4}
      pathOptions={{ color: "#3b82f6", className: "tripviewer-route" }} eventHandlers={{ click }} bubblingMouseEvents={false} />
  ) : (
    <Polyline key={index} positions={positions}
      pathOptions={{ className: "tripviewer-route", color: "#3b82f6", weight: 5, opacity: 0.7 }}
      eventHandlers={{ click }} bubblingMouseEvents={false} />
  ))}</>;
}
