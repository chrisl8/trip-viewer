import { useEffect, useMemo, useRef } from "react";
import { CircleMarker, useMap } from "react-leaflet";
import { useStore } from "../../state/store";
import { interpolateGps } from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";

interface Props {
  gpsPoints: GpsPoint[];
  /** Whole-trip GPS (all segments stitched), used to fit-bounds on
   *  trip select. Distinct from `gpsPoints`, which in Original mode
   *  is just the active segment's track and would fit-bounds to a
   *  small window around the start of the trip. */
  tripGpsPoints: GpsPoint[];
  /** Time to interpolate at. The caller decides whether this is
   *  segment-local seconds (matching `gpsPoints` from the active
   *  segment) in Original mode, or concat-time seconds (matching
   *  the trip-stitched `gpsPoints`) in tiered mode. */
  interpolationTime: number;
  activeSegment: Segment | null;
}

export function VehicleMarker({
  gpsPoints,
  tripGpsPoints,
  interpolationTime,
  activeSegment,
}: Props) {
  const map = useMap();
  const loadedTripId = useStore((s) => s.loadedTripId);
  const isPlaying = useStore((s) => s.isPlaying);
  const fittedTripRef = useRef<string | null>(null);
  const zoomedInTripRef = useRef<string | null>(null);
  const userInteractingRef = useRef(false);

  const interp = useMemo(
    () =>
      activeSegment ? interpolateGps(gpsPoints, interpolationTime) : null,
    [gpsPoints, interpolationTime, activeSegment],
  );

  // Reset one-shots when the trip changes so the new trip gets its
  // own fit-bounds + first-play zoom-in.
  useEffect(() => {
    fittedTripRef.current = null;
    zoomedInTripRef.current = null;
  }, [loadedTripId]);

  // Track whether the user is currently dragging or zooming the map.
  // Pan-follow defers to active gestures — yanking the view mid-drag
  // would feel like the app fighting the user.
  useEffect(() => {
    const onStart = () => {
      userInteractingRef.current = true;
    };
    const onEnd = () => {
      userInteractingRef.current = false;
    };
    map.on("dragstart", onStart);
    map.on("dragend", onEnd);
    map.on("zoomstart", onStart);
    map.on("zoomend", onEnd);
    return () => {
      map.off("dragstart", onStart);
      map.off("dragend", onEnd);
      map.off("zoomstart", onStart);
      map.off("zoomend", onEnd);
    };
  }, [map]);

  // Fit-bounds once per trip, as soon as trip-level GPS is available.
  // maxZoom caps the initial view so very short trips don't end up
  // zoomed in tighter than the eventual vehicle-level zoom.
  useEffect(() => {
    if (!loadedTripId || fittedTripRef.current === loadedTripId) return;
    if (tripGpsPoints.length === 0) return;

    fittedTripRef.current = loadedTripId;
    // Force a fresh size read before fitBounds. Leaflet's cached
    // _size can be stale on the first fit after mount, which makes
    // fitBounds compute the wrong center+zoom for the actual viewport.
    map.invalidateSize();
    const lats = tripGpsPoints.map((p) => p.lat);
    const lons = tripGpsPoints.map((p) => p.lon);
    map.fitBounds(
      [
        [Math.min(...lats), Math.min(...lons)],
        [Math.max(...lats), Math.max(...lons)],
      ],
      { padding: [30, 30], maxZoom: 15 },
    );
  }, [loadedTripId, tripGpsPoints, map]);

  // First-play zoom-in: snap to the vehicle once per trip when
  // playback first starts and we have a usable interpolated point.
  // Subsequent plays don't re-zoom — the user owns zoom from here.
  // Intentionally per-trip, NOT per-segment: switching segments
  // within a trip lets pan-follow bring the new vehicle position
  // into view at whatever zoom the user has chosen, instead of
  // snapping back to 15× and overriding their context view.
  useEffect(() => {
    if (!loadedTripId || zoomedInTripRef.current === loadedTripId) return;
    if (!isPlaying) return;
    if (!interp || interp.stale) return;

    zoomedInTripRef.current = loadedTripId;
    map.setView([interp.lat, interp.lon], 15, { animate: true });
  }, [loadedTripId, isPlaying, interp, map]);

  // Pan-follow whenever the marker leaves the visible area. Pan and
  // zoom are independent — if the user has zoomed out, this just
  // keeps following at their chosen zoom. Skipped while the user is
  // mid-drag or mid-zoom so the auto-pan doesn't fight the gesture;
  // the next interp tick after gesture-end will catch up if needed.
  useEffect(() => {
    if (!interp || interp.stale) return;
    if (userInteractingRef.current) return;
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
