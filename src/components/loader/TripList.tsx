import { useState } from "react";
import clsx from "clsx";
import { extractGpsBatch } from "../../ipc/gps";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";
import { TripBadges } from "../sidebar/TripBadges";
import { TripActionsMenu } from "../trip/TripActionsMenu";
import { MergeTripsDialog } from "../trip/MergeTripsDialog";
import { formatTripStart } from "../../utils/format";

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
  const markedForMerge = useStore((s) => s.markedForMerge);
  const clearMergeMarks = useStore((s) => s.clearMergeMarks);
  const [showMergeDialog, setShowMergeDialog] = useState(false);

  const markedTrips = trips.filter((t) => markedForMerge.has(t.id));
  const canMerge = markedTrips.length >= 2;

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
    <>
      {markedForMerge.size > 0 && (
        <div
          className={clsx(
            "mx-2 mt-2 flex items-center gap-2 rounded-md border px-2 py-1.5 text-xs",
            canMerge
              ? "border-sky-700 bg-sky-950/60 text-sky-200"
              : "border-neutral-700 bg-neutral-900 text-neutral-400",
          )}
        >
          <span className="flex-1">
            {markedForMerge.size}{" "}
            {markedForMerge.size === 1 ? "trip" : "trips"} marked
            {!canMerge && " · need 2+ to merge"}
          </span>
          <button
            onClick={() => setShowMergeDialog(true)}
            disabled={!canMerge}
            className={clsx(
              "rounded px-2 py-0.5 font-medium",
              canMerge
                ? "bg-sky-700 text-white hover:bg-sky-600"
                : "cursor-not-allowed bg-neutral-800 text-neutral-500",
            )}
            title={
              canMerge
                ? "Merge the marked trips into one"
                : "Mark at least one more trip to enable merge"
            }
          >
            Merge
          </button>
          <button
            onClick={() => clearMergeMarks()}
            className="rounded px-2 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
            title="Clear all marks"
          >
            Clear
          </button>
        </div>
      )}

      <ul className="flex flex-col gap-1 overflow-y-auto p-2">
        {trips.map((trip) => {
          const active = trip.id === selectedTripId;
          const archive = trip.archiveOnly === true;
          const marked = markedForMerge.has(trip.id);
          return (
            <li key={trip.id}>
              <div
                className={clsx(
                  "group relative flex items-start rounded-md transition-colors",
                  active
                    ? "bg-neutral-700 text-white"
                    : "text-neutral-300 hover:bg-neutral-800",
                  // Marked trips get a sky-blue ring so the user can
                  // see the selection at a glance, even as they scroll
                  // away from the kebab they used to mark.
                  marked && "ring-1 ring-inset ring-sky-500",
                )}
              >
                <button
                  onClick={() => void onSelectTrip(trip.id)}
                  className="flex-1 px-3 py-2 text-left text-sm"
                >
                  <div className="flex items-center gap-2 pr-7 font-medium">
                    <span>{formatTripStart(trip.startTime)}</span>
                    {archive && (
                      <span
                        className="rounded-sm bg-amber-900/60 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-amber-200"
                        title="Source files have been deleted; only the timelapse archive remains."
                      >
                        Archive
                      </span>
                    )}
                    {marked && (
                      <span
                        className="rounded-sm bg-sky-900/60 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-sky-200"
                        title="Marked for merge"
                      >
                        Merge
                      </span>
                    )}
                  </div>
                  <div className="text-xs text-neutral-500">
                    {archive
                      ? "Timelapse only"
                      : `${trip.segments.length} segments · ${formatDuration(trip)}`}
                  </div>
                  <TripBadges tripId={trip.id} />
                </button>
                <div className="absolute right-1 top-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
                  <TripActionsMenu trip={trip} />
                </div>
              </div>
            </li>
          );
        })}
      </ul>

      {showMergeDialog && canMerge && (
        <MergeTripsDialog
          marked={markedTrips}
          onClose={() => setShowMergeDialog(false)}
        />
      )}
    </>
  );
}
