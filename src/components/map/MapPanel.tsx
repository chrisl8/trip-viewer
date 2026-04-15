import { useMemo } from "react";
import { MapContainer, TileLayer } from "react-leaflet";
import { useStore } from "../../state/store";
import { interpolateGps } from "../../engine/interpolate";
import type { GpsPoint, Segment } from "../../types/model";
import { VehicleMarker } from "./VehicleMarker";
import { TrackPolyline } from "./TrackPolyline";
import { SpeedReadout } from "../hud/SpeedReadout";
import { HeadingReadout } from "../hud/HeadingReadout";
import "leaflet/dist/leaflet.css";

interface Props {
  activeSegment: Segment | null;
}

function GpsMissingRibbon({ gpsPoints, activeSegment }: { gpsPoints: GpsPoint[]; activeSegment: Segment | null }) {
  const currentTime = useStore((s) => s.currentTime);

  const interp = useMemo(
    () => (activeSegment ? interpolateGps(gpsPoints, currentTime) : null),
    [gpsPoints, currentTime, activeSegment],
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

  const center = useMemo((): [number, number] => {
    if (tripGpsPoints.length > 0) {
      const mid = tripGpsPoints[Math.floor(tripGpsPoints.length / 2)];
      return [mid.lat, mid.lon];
    }
    return [37.69, -97.34];
  }, [tripGpsPoints]);

  if (tripGpsPoints.length === 0) {
    return (
      <div className="flex h-full items-center justify-center rounded-md bg-neutral-900 text-xs text-neutral-500">
        No GPS data
      </div>
    );
  }

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
        <TrackPolyline points={tripGpsPoints} />
        <VehicleMarker
          gpsPoints={segmentGpsPoints}
          activeSegment={activeSegment}
        />
      </MapContainer>

      <GpsMissingRibbon
        gpsPoints={segmentGpsPoints}
        activeSegment={activeSegment}
      />

      <div className="pointer-events-none absolute bottom-3 right-3 z-[1000] flex gap-2">
        <SpeedReadout gpsPoints={segmentGpsPoints} activeSegment={activeSegment} />
        <HeadingReadout gpsPoints={segmentGpsPoints} activeSegment={activeSegment} />
      </div>
    </div>
  );
}
