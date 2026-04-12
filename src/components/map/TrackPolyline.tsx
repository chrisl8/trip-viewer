import { useMemo } from "react";
import { Polyline } from "react-leaflet";
import type { GpsPoint } from "../../types/model";
import type { LatLngExpression } from "leaflet";

interface Props {
  points: GpsPoint[];
}

export function TrackPolyline({ points }: Props) {
  const positions: LatLngExpression[] = useMemo(
    () => points.map((p) => [p.lat, p.lon] as [number, number]),
    [points],
  );

  if (positions.length < 2) return null;

  return (
    <Polyline
      positions={positions}
      pathOptions={{ color: "#3b82f6", weight: 3, opacity: 0.7 }}
    />
  );
}
