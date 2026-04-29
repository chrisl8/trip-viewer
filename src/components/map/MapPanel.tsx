import { useEffect, useMemo, useState } from "react";
import { MapContainer, TileLayer, useMap } from "react-leaflet";
import { useStore } from "../../state/store";
import { interpolateGps } from "../../engine/interpolate";
import { dumpMiltonaGpsDebug } from "../../ipc/gps";
import type { GpsPoint, Segment } from "../../types/model";
import { VehicleMarker } from "./VehicleMarker";
import { TrackPolyline } from "./TrackPolyline";
import { SpeedReadout } from "../hud/SpeedReadout";
import { HeadingReadout } from "../hud/HeadingReadout";
import { computeTripTime } from "../../utils/tripTime";
import "leaflet/dist/leaflet.css";

/**
 * Keeps Leaflet's cached container size in sync with the actual DOM
 * size. Without this, fitBounds called shortly after mount can land on
 * the wrong center+zoom because Leaflet computes the result against a
 * stale `_size`. Symptom: map shows the trip area but shifted/zoomed
 * wrong until a click-away-and-back forces a remount.
 */
function MapResizeObserver() {
  const map = useMap();
  useEffect(() => {
    const container = map.getContainer();
    const ro = new ResizeObserver(() => map.invalidateSize());
    ro.observe(container);
    return () => ro.disconnect();
  }, [map]);
  return null;
}

/**
 * Button for Miltona segments that dumps the raw `gps0` atom plus every
 * candidate lat/lon decoding to a text file. Only visible while we're still
 * finalizing the Miltona GPS format — a tester at a known location runs
 * this and sends back the output so we can lock in the right scaling.
 */
function MiltonaDebugButton({ segment }: { segment: Segment }) {
  const [status, setStatus] = useState<
    { kind: "idle" }
    | { kind: "running" }
    | { kind: "done"; path: string }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

  if (segment.cameraKind !== "miltona") return null;
  const path = segment.channels[0]?.filePath;
  if (!path) return null;

  async function onClick() {
    setStatus({ kind: "running" });
    try {
      const out = await dumpMiltonaGpsDebug(path!);
      setStatus({ kind: "done", path: out });
    } catch (e) {
      setStatus({ kind: "error", message: String(e) });
    }
  }

  return (
    <div className="pointer-events-auto absolute bottom-3 left-3 z-[1000] max-w-[60%]">
      <button
        type="button"
        onClick={() => void onClick()}
        disabled={status.kind === "running"}
        className="rounded-md bg-neutral-900/90 px-2 py-1 text-xs text-neutral-200 shadow-lg hover:bg-neutral-800 disabled:opacity-60"
        title="Miltona GPS decoding is provisional. This writes a diagnostic file you can send back to help finalize the format."
      >
        {status.kind === "running" ? "Exporting…" : "Export GPS debug"}
      </button>
      {status.kind === "done" && (
        <div className="mt-1 break-all rounded-md bg-neutral-900/90 px-2 py-1 text-[10px] text-neutral-300 shadow-lg">
          Wrote {status.path}
        </div>
      )}
      {status.kind === "error" && (
        <div className="mt-1 rounded-md bg-red-950/90 px-2 py-1 text-[10px] text-red-200 shadow-lg">
          {status.message}
        </div>
      )}
    </div>
  );
}

interface Props {
  activeSegment: Segment | null;
}

function GpsMissingRibbon({
  gpsPoints,
  interpolationTime,
  activeSegment,
}: {
  gpsPoints: GpsPoint[];
  interpolationTime: number;
  activeSegment: Segment | null;
}) {
  const interp = useMemo(
    () =>
      activeSegment ? interpolateGps(gpsPoints, interpolationTime) : null,
    [gpsPoints, interpolationTime, activeSegment],
  );

  if (!activeSegment) return null;

  const noGps = gpsPoints.length === 0;
  const stale = interp?.stale === true;

  if (!noGps && !stale) return null;

  return (
    <div className="pointer-events-none absolute left-0 right-0 top-0 z-[1000] bg-yellow-900/80 px-3 py-1 text-center text-xs text-yellow-200">
      {noGps ? "No GPS data for this segment" : "GPS data unavailable at current position"}
    </div>
  );
}

export function MapPanel({ activeSegment }: Props) {
  const gpsByFile = useStore((s) => s.gpsByFile);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const trips = useStore((s) => s.trips);
  const currentTime = useStore((s) => s.currentTime);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const sourceMode = useStore((s) => s.sourceMode);
  const activeSpeedCurve = useStore((s) => s.activeSpeedCurve);

  // Interpolation inputs vary by source:
  //  - Original: per-segment GPS + segment-local currentTime
  //  - Tiered:   trip-stitched GPS + concat-time derived via the curve
  //
  // The readouts and markers are agnostic to which axis is in play;
  // MapPanel picks the right pair once and propagates it.
  const trip = trips.find((t) => t.id === loadedTripId);
  const isTiered = sourceMode !== "original";
  const concatTime = isTiered
    ? computeTripTime(trip, activeSegmentId, currentTime, sourceMode, activeSpeedCurve)
    : 0;

  const tripGpsPoints: GpsPoint[] = useMemo(() => {
    const trip = trips.find((t) => t.id === loadedTripId);
    if (!trip) return [];
    const all: GpsPoint[] = [];
    let cumulativeOffset = 0;
    for (const seg of trip.segments) {
      // Master channel (first in canonical order) carries GPS.
      const front = seg.channels[0];
      if (!front) continue;
      const pts = gpsByFile[front.filePath];
      if (pts) {
        for (const p of pts) {
          all.push({ ...p, tOffsetS: cumulativeOffset + p.tOffsetS });
        }
      }
      cumulativeOffset += seg.durationS;
    }
    return all;
  }, [trips, loadedTripId, gpsByFile]);

  const segmentGpsPoints: GpsPoint[] = useMemo(() => {
    if (!activeSegment) return [];
    const front = activeSegment.channels[0];
    if (!front) return [];
    return gpsByFile[front.filePath] ?? [];
  }, [activeSegment, gpsByFile]);

  // Pick which pair feeds the marker + readouts: tiered mode uses the
  // full trip trace indexed by concat-time; Original uses per-segment
  // GPS indexed by segment-local time.
  const consumerGpsPoints = isTiered ? tripGpsPoints : segmentGpsPoints;
  const consumerInterpTime = isTiered ? concatTime : currentTime;

  const center = useMemo((): [number, number] => {
    if (tripGpsPoints.length > 0) {
      const mid = tripGpsPoints[Math.floor(tripGpsPoints.length / 2)];
      return [mid.lat, mid.lon];
    }
    return [37.69, -97.34];
  }, [tripGpsPoints]);

  // If the camera model doesn't record GPS at all, don't render anything —
  // PlayerShell collapses the grid slot and shows a small inline caption
  // instead of leaving an empty panel. See PlayerShell.tsx for the layout
  // branch that reacts to this.
  if (activeSegment && !activeSegment.gpsSupported) {
    return null;
  }

  // MapContainer is always mounted while a trip is loaded so that
  // GPS arriving after a trip click doesn't trigger a fresh mount of
  // Leaflet (which races with the Polyline/CircleMarker children's
  // pane attachment — symptom: tiles render but track + marker
  // silently fail to paint until you click away and back). The
  // "No GPS data" caption renders on top instead of replacing it.
  const hasTripGps = tripGpsPoints.length > 0;

  return (
    <div className="relative h-full w-full overflow-hidden rounded-md">
      <MapContainer
        center={center}
        zoom={15}
        className="h-full w-full"
        zoomControl={false}
        attributionControl={false}
      >
        <TileLayer url="https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png" />
        <MapResizeObserver />
        {hasTripGps && <TrackPolyline points={tripGpsPoints} />}
        {hasTripGps && (
          <VehicleMarker
            gpsPoints={consumerGpsPoints}
            tripGpsPoints={tripGpsPoints}
            interpolationTime={consumerInterpTime}
            activeSegment={activeSegment}
          />
        )}
      </MapContainer>

      {!hasTripGps && (
        <div className="pointer-events-none absolute inset-0 z-[1000] flex items-center justify-center bg-neutral-900/70 text-xs text-neutral-400">
          Loading GPS…
        </div>
      )}

      <GpsMissingRibbon
        gpsPoints={consumerGpsPoints}
        interpolationTime={consumerInterpTime}
        activeSegment={activeSegment}
      />

      {activeSegment && <MiltonaDebugButton segment={activeSegment} />}

      <div className="pointer-events-none absolute bottom-3 right-3 z-[1000] flex gap-2">
        <SpeedReadout
          gpsPoints={consumerGpsPoints}
          interpolationTime={consumerInterpTime}
          activeSegment={activeSegment}
        />
        <HeadingReadout
          gpsPoints={consumerGpsPoints}
          interpolationTime={consumerInterpTime}
          activeSegment={activeSegment}
        />
      </div>
    </div>
  );
}
