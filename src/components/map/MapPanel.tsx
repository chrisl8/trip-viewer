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
 * Parse a camera-local NaiveDateTime without applying the computer's timezone.
 * We only need arithmetic between segment timestamps recorded by the same
 * camera, so treating the numeric fields as a timezone-free UTC tuple avoids
 * introducing host timezone/DST behaviour into the calculation.
 */
function naiveDateTimeMs(value: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?/.exec(
    value,
  );
  if (!match) return null;

  const fractionMs = Number(`0.${match[7] ?? "0"}`) * 1000;
  return Date.UTC(
    Number(match[1]),
    Number(match[2]) - 1,
    Number(match[3]),
    Number(match[4]),
    Number(match[5]),
    Number(match[6]),
    Math.round(fractionMs),
  );
}

/**
 * Rebase GPS samples from the preceding segment onto the current segment's
 * local time axis and keep only samples whose actual timestamp reaches into
 * the current segment. No grace period is assumed: continuity exists only
 * when the recorded timestamps themselves overlap the boundary.
 */
function precedingGpsOverlap(
  previousSegment: Segment,
  currentSegment: Segment,
  previousPoints: GpsPoint[],
): GpsPoint[] {
  const previousStartMs = naiveDateTimeMs(previousSegment.startTime);
  const currentStartMs = naiveDateTimeMs(currentSegment.startTime);
  if (previousStartMs === null || currentStartMs === null) return [];

  const startDeltaS = (previousStartMs - currentStartMs) / 1000;
  return previousPoints
    .map((point) => ({
      ...point,
      tOffsetS: startDeltaS + point.tOffsetS,
    }))
    .filter((point) => point.tOffsetS >= 0);
}

/**
 * Merge overlapping GPS samples by their exact segment-relative timestamp.
 * When both files contain the same timestamp, prefer the current segment's
 * sample because it belongs to the video that is now active.
 */
function mergeGpsAtBoundary(
  precedingOverlap: GpsPoint[],
  currentPoints: GpsPoint[],
): GpsPoint[] {
  if (precedingOverlap.length === 0) return currentPoints;

  const byTimestamp = new Map<number, GpsPoint>();
  for (const point of precedingOverlap) byTimestamp.set(point.tOffsetS, point);
  for (const point of currentPoints) byTimestamp.set(point.tOffsetS, point);
  return Array.from(byTimestamp.values()).sort((a, b) => a.tOffsetS - b.tOffsetS);
}

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
  onSeekTripTime: (time: number) => void;
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
  const beforeFirstFix =
    gpsPoints.length > 0 && interpolationTime < gpsPoints[0].tOffsetS;
  const stale = beforeFirstFix || interp?.stale === true;

  if (!noGps && !stale) return null;

  return (
    <div className="pointer-events-none absolute left-0 right-0 top-0 z-[1000] bg-yellow-900/80 px-3 py-1 text-center text-xs text-yellow-200">
      {noGps ? "No GPS data for this segment" : "GPS data unavailable at current position"}
    </div>
  );
}

export function MapPanel({ activeSegment, onSeekTripTime }: Props) {
  const gpsByFile = useStore((s) => s.gpsByFile);
  const tripGpsByTrip = useStore((s) => s.tripGpsByTrip);
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
    // Archived: trip-stitched GPS from migration 0012's trip_gps table.
    // Already cumulative; works for archive-only trips and is cheaper
    // than per-file extraction even when originals exist.
    const archived = loadedTripId ? tripGpsByTrip[loadedTripId] : undefined;
    if (archived && archived.length > 0) return archived;
    // Legacy/in-memory fallback. Prefer whichever camera in the segment
    // actually has GPS rather than assuming the canonical Front channel.
    const all: GpsPoint[] = [];
    let cumulativeOffset = 0;
    for (const seg of trip.segments) {
      const provider = seg.channels.find(
        (channel) => (gpsByFile[channel.filePath]?.length ?? 0) > 0,
      );
      const pts = provider ? gpsByFile[provider.filePath] : undefined;
      if (pts) {
        for (const p of pts) {
          all.push({ ...p, tOffsetS: cumulativeOffset + p.tOffsetS });
        }
      }
      cumulativeOffset += seg.durationS;
    }
    return all;
  }, [trips, loadedTripId, gpsByFile, tripGpsByTrip]);

  const segmentGpsPoints: GpsPoint[] = useMemo(() => {
    if (!activeSegment) return [];
    if (!trip) return [];

    const segmentIndex = trip.segments.findIndex(
      (segment) => segment.id === activeSegment.id,
    );
    if (segmentIndex < 0) return [];

    // Prefer the persisted trip-wide cache even in Original mode. Convert the
    // trip-time offsets back onto this segment's local axis. This removes the
    // old need to re-parse every source MP4 whenever the app is restarted.
    const archived = loadedTripId ? tripGpsByTrip[loadedTripId] : undefined;
    if (archived) {
      let segmentStart = 0;
      for (let i = 0; i < segmentIndex; i++) {
        segmentStart += trip.segments[i].durationS;
      }
      const segmentEnd = segmentStart + activeSegment.durationS;
      return archived
        .filter(
          (point) =>
            point.tOffsetS >= segmentStart && point.tOffsetS <= segmentEnd,
        )
        .map((point) => ({
          ...point,
          tOffsetS: point.tOffsetS - segmentStart,
        }));
    }

    // Compatibility fallback for an extraction that has not yet reached the
    // DB. Use the first camera with usable GPS, not necessarily Front.
    const provider = activeSegment.channels.find(
      (channel) => (gpsByFile[channel.filePath]?.length ?? 0) > 0,
    );
    const currentPoints = provider ? gpsByFile[provider.filePath] ?? [] : [];

    if (segmentIndex <= 0) return currentPoints;

    const previousSegment = trip.segments[segmentIndex - 1];
    const previousProvider = previousSegment.channels.find(
      (channel) => (gpsByFile[channel.filePath]?.length ?? 0) > 0,
    );
    if (!previousProvider) return currentPoints;

    const previousPoints = gpsByFile[previousProvider.filePath] ?? [];
    const overlap = precedingGpsOverlap(
      previousSegment,
      activeSegment,
      previousPoints,
    );
    return mergeGpsAtBoundary(overlap, currentPoints);
  }, [
    activeSegment,
    gpsByFile,
    loadedTripId,
    trip,
    tripGpsByTrip,
  ]);

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
        {hasTripGps && <TrackPolyline points={tripGpsPoints} currentTripTime={concatTime} onSeekTripTime={onSeekTripTime} />}
        {hasTripGps && (
          <VehicleMarker
            gpsPoints={consumerGpsPoints}
            tripGpsPoints={tripGpsPoints}
            interpolationTime={consumerInterpTime}
            activeSegment={activeSegment}
          />
        )}
      </MapContainer>

      {hasTripGps && <div className="pointer-events-none absolute left-3 top-3 z-[1000] rounded bg-black/60 px-2 py-1 text-[10px] text-neutral-200">Click route to seek video</div>}
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
