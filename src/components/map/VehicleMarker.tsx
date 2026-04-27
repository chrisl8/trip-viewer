import { useEffect, useMemo, useRef } from "react";
import { CircleMarker, useMap } from "react-leaflet";
import { useStore } from "../../state/store";
import { interpolateGps } from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";

interface Props {
  gpsPoints: GpsPoint[];
  /** Time to interpolate at. The caller decides whether this is
   *  segment-local seconds (matching `gpsPoints` from the active
   *  segment) in Original mode, or concat-time seconds (matching
   *  the trip-stitched `gpsPoints`) in tiered mode. */
  interpolationTime: number;
  activeSegment: Segment | null;
}

export function VehicleMarker({
  gpsPoints,
  interpolationTime,
  activeSegment,
}: Props) {
  const map = useMap();
  const hasFitRef = useRef<string | null>(null);

  const interp = useMemo(
    () =>
      activeSegment ? interpolateGps(gpsPoints, interpolationTime) : null,
    [gpsPoints, interpolationTime, activeSegment],
  );

  // Fit map bounds on first GPS load per trip
  useEffect(() => {
    const tripId = useStore.getState().loadedTripId;
    if (!tripId || hasFitRef.current === tripId) return;
    if (gpsPoints.length === 0) return;

    hasFitRef.current = tripId;
    const lats = gpsPoints.map((p) => p.lat);
    const lons = gpsPoints.map((p) => p.lon);
    map.fitBounds([
      [Math.min(...lats), Math.min(...lons)],
      [Math.max(...lats), Math.max(...lons)],
    ], { padding: [30, 30] });
  }, [gpsPoints, map]);

  // Follow the marker during playback
  useEffect(() => {
    if (!interp || interp.stale) return;
    if (!map.getBounds().contains([interp.lat, interp.lon])) {
      map.panTo([interp.lat, interp.lon], { animate: true, duration: 0.3 });
    }
  }, [interp, map]);

  if (!interp) return null;

  return (
    <CircleMarker
      center={[interp.lat, interp.lon]}
      radius={8}
      pathOptions={{
        color: interp.stale ? "#6b7280" : "#ef4444",
        fillColor: interp.stale ? "#6b7280" : "#ef4444",
        fillOpacity: interp.stale ? 0.4 : 0.9,
        weight: 2,
      }}
    />
  );
}
