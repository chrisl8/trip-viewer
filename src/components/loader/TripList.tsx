import clsx from "clsx";
import { extractGpsBatch } from "../../ipc/gps";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";
import { TripBadges } from "../sidebar/TripBadges";

function formatTripLabel(trip: Trip): string {
  const start = new Date(trip.startTime);
  const date = start.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
  const time = start.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${date} ${time}`;
}

function formatDuration(trip: Trip): string {
  const total = trip.segments.reduce((sum, s) => sum + s.durationS, 0);
  const mins = Math.floor(total / 60);
  const secs = Math.round(total % 60);
  return `${mins}m ${secs}s`;
}

export function TripList() {
  const trips = useStore((s) => s.trips);
  const selectedTripId = useStore((s) => s.selectedTripId);
  const selectTrip = useStore((s) => s.selectTrip);

  async function onSelectTrip(tripId: string) {
    selectTrip(tripId);
    const trip = useStore.getState().trips.find((t) => t.id === tripId);
    if (!trip) return;
    // GPS data lives with the first/master channel (Front on Wolf Box,
    // first in canonical order otherwise). We pair each path with its
    // segment's cameraKind so the backend dispatches to the right decoder
    // (Wolf Box's ShenShu meta-track vs. Miltona's gps0 atom vs. Thinkware's
    // none-at-all).
    const requests = trip.segments
      .map((s) => {
        const path = s.channels[0]?.filePath;
        if (!path || !s.gpsSupported) return null;
        return { path, cameraKind: s.cameraKind };
      })
      .filter((r): r is { path: string; cameraKind: typeof trip.segments[0]["cameraKind"] } => r !== null);
    if (requests.length === 0) return;
    try {
      const results = await extractGpsBatch(requests);
      const gpsByFile = { ...useStore.getState().gpsByFile };
      for (const item of results) {
        gpsByFile[item.filePath] = item.points;
      }
      useStore.setState({ gpsByFile });
    } catch (e) {
      console.error("GPS extraction failed:", e);
    }
  }

  if (trips.length === 0) {
    return (
      <p className="px-3 py-4 text-sm text-neutral-500">
        No trips loaded. Open a folder to begin.
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-1 overflow-y-auto p-2">
      {trips.map((trip) => {
        const active = trip.id === selectedTripId;
        return (
          <li key={trip.id}>
            <button
              onClick={() => void onSelectTrip(trip.id)}
              className={clsx(
                "w-full rounded-md px-3 py-2 text-left text-sm transition-colors",
                active
                  ? "bg-neutral-700 text-white"
                  : "text-neutral-300 hover:bg-neutral-800",
              )}
            >
              <div className="font-medium">{formatTripLabel(trip)}</div>
              <div className="text-xs text-neutral-500">
                {trip.segments.length} segments · {formatDuration(trip)}
              </div>
              <TripBadges tripId={trip.id} />
            </button>
          </li>
        );
      })}
    </ul>
  );
}
