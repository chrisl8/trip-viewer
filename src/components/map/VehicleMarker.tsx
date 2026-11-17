import { useCallback, useEffect, useMemo, useRef } from "react";
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

  // First-play zoom-in: establish the vehicle-level view once per trip as
  // soon as playback starts. If playback begins before the first GPS fix,
  // use the trip's first recorded point rather than waiting for that fix to
  // arrive mid-playback. On WebKitGTK the old behaviour could trigger a
  // Leaflet zoom animation at exactly the moment the first GPS point became
  // live, adding a compositor spike while the video pipelines were decoding.
  // Use a non-animated setView here; normal follow-mode below may animate later
  // once playback is established.
  useEffect(() => {
    if (!loadedTripId || zoomedInTripRef.current === loadedTripId) return;
    if (!isPlaying) return;

    const target =
      interp && !interp.stale
        ? ([interp.lat, interp.lon] as [number, number])
        : tripGpsPoints.length > 0
          ? ([tripGpsPoints[0].lat, tripGpsPoints[0].lon] as [number, number])
          : null;
    if (!target) return;

    zoomedInTripRef.current = loadedTripId;
    map.setView(target, 15, { animate: false });
  }, [loadedTripId, isPlaying, interp, tripGpsPoints, map]);

  // Keep the vehicle close to the visual center without panning on every
  // GPS sample. The marker may move inside a small square dead zone around
  // the center; once it leaves that square we pan it back to the exact center
  // at the user's current zoom. The dead-zone side is 12% of the shorter map
  // edge, clamped to 60–100 px, so resized panels remain useful without
  // turning follow-mode into continuous animation.
  //
  // ResizeObserver in MapPanel calls invalidateSize(), which emits Leaflet's
  // resize event; we run the same check there so resizing the map can trigger
  // an immediate follow correction without waiting for the next GPS sample.
  const keepMarkerNearCenter = useCallback(() => {
    if (!interp || interp.stale) return;
    if (userInteractingRef.current) return;

    const size = map.getSize();
    if (size.x <= 0 || size.y <= 0) return;

    const point = map.latLngToContainerPoint([interp.lat, interp.lon]);
    const centerX = size.x / 2;
    const centerY = size.y / 2;
    const deadZoneSide = Math.min(
      100,
      Math.max(60, Math.min(size.x, size.y) * 0.12),
    );
    const half = deadZoneSide / 2;
    const outsideDeadZone =
      Math.abs(point.x - centerX) > half ||
      Math.abs(point.y - centerY) > half;

    if (outsideDeadZone) {
      map.panTo([interp.lat, interp.lon], { animate: true, duration: 0.25 });
    }
  }, [interp, map]);

  useEffect(() => {
    keepMarkerNearCenter();
  }, [keepMarkerNearCenter]);

  useEffect(() => {
    map.on("resize", keepMarkerNearCenter);
    return () => {
      map.off("resize", keepMarkerNearCenter);
    };
  }, [map, keepMarkerNearCenter]);

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
