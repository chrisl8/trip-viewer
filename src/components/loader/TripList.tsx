import { useMemo, useState } from "react";
import clsx from "clsx";
import { prepareTripGps } from "../../state/gpsPreparation";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";
import type { TimelapseJobRow } from "../../ipc/timelapse";
import { TripBadges } from "../sidebar/TripBadges";
import { TripActionsMenu } from "../trip/TripActionsMenu";
import { MergeTripsDialog } from "../trip/MergeTripsDialog";
import { BulkDeleteTripsDialog } from "../review/BulkDeleteTripsDialog";
import { formatBytes, formatTripStart } from "../../utils/format";

function formatDuration(trip: Trip): string {
  const total = trip.segments.reduce((sum, s) => sum + s.durationS, 0);
  const mins = Math.floor(total / 60);
  const secs = Math.round(total % 60);
  return `${mins}m ${secs}s`;
}

/**
 * Sum segment sizes for a trip. Returns `null` when *any* segment has
 * an unknown size — partial totals would mislead, so we'd rather show
 * "—" until the next scan stamps the missing rows.
 */
function tripOriginalsBytes(trip: Trip): number | null {
  if (trip.segments.length === 0) return null;
  let total = 0;
  for (const seg of trip.segments) {
    if (seg.sizeBytes == null) return null;
    total += seg.sizeBytes;
  }
  return total;
}

function tripTimelapseBytes(
  tripId: string,
  jobs: TimelapseJobRow[],
): number | null {
  const tripJobs = jobs.filter(
    (j) => j.tripId === tripId && j.outputSizeBytes != null,
  );
  if (tripJobs.length === 0) return null;
  return tripJobs.reduce((sum, j) => sum + (j.outputSizeBytes ?? 0), 0);
}

/** 3 tiers (8x/16x/60x) × 3 channels (F/I/R) — the full set of renders
 *  a trip can have. Matches the fixed universe the timelapse encoder
 *  and merge backend both key off of. */
const TOTAL_TIMELAPSE_TUPLES = 9;

/** Count of distinct (tier, channel) tuples with a `done` render for
 *  this trip, out of `TOTAL_TIMELAPSE_TUPLES` possible. There's no
 *  sidebar signal for this otherwise — a merge can silently cost you
 *  finished renders with nothing here to warn you first. */
function tripTimelapseCoverage(
  tripId: string,
  jobs: TimelapseJobRow[],
): number {
  const done = new Set<string>();
  for (const j of jobs) {
    if (j.tripId === tripId && j.status === "done") {
      done.add(`${j.tier}/${j.channel}`);
    }
  }
  return done.size;
}

export function TripList() {
  const trips = useStore((s) => s.trips);
  const selectedTripId = useStore((s) => s.selectedTripId);
  const selectTrip = useStore((s) => s.selectTrip);
  const selectedTripIds = useStore((s) => s.selectedTripIds);
  const toggleTripSelection = useStore((s) => s.toggleTripSelection);
  const clearTripSelection = useStore((s) => s.clearTripSelection);
  const deleteTripsCompletely = useStore((s) => s.deleteTripsCompletely);
  const deleteOriginalsForTrips = useStore((s) => s.deleteOriginalsForTrips);
  const timelapseJobs = useStore((s) => s.timelapseJobs);
  const reclaimableFilter = useStore((s) => s.reclaimableFilter);
  const setReclaimableFilter = useStore((s) => s.setReclaimableFilter);
  const reclaimableIds = useStore(
    (s) => s.librarySummary?.reclaimableTripIds ?? null,
  );
  const libraryFirstLoadDone = useStore((s) => s.libraryFirstLoadDone);
  const [showMergeDialog, setShowMergeDialog] = useState(false);
  // Bulk-delete dialog: which mode, plus a snapshot of the targeted trips
  // captured at open time so the summary stays stable even after the
  // store action clears the live selection on completion.
  const [bulkMode, setBulkMode] = useState<null | "trip" | "originals">(null);
  const [bulkTrips, setBulkTrips] = useState<Trip[]>([]);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkError, setBulkError] = useState<string | null>(null);

  const selectedTrips = trips.filter((t) => selectedTripIds.has(t.id));
  const canMerge = selectedTrips.length >= 2;
  const originalsSelectable = selectedTrips.filter((t) => !t.archiveOnly);

  const visibleTrips = useMemo(() => {
    if (!reclaimableFilter || !reclaimableIds) return trips;
    const allow = new Set(reclaimableIds);
    return trips.filter((t) => allow.has(t.id));
  }, [trips, reclaimableFilter, reclaimableIds]);

  async function onSelectTrip(tripId: string) {
    selectTrip(tripId);
    const trip = useStore.getState().trips.find((t) => t.id === tripId);
    if (!trip) return;

    // GPS is now a real library cache, not a per-view calculation. The backend
    // returns the current trip_gps row when available; otherwise it extracts
    // once from the segment camera files, chooses the strongest usable camera
    // source per segment, persists the stitched result, and returns it.
    try {
      await prepareTripGps(trip);
    } catch (e) {
      console.error("ensureTripGps failed:", e);
    }
  }

  function onRowClick(e: React.MouseEvent, tripId: string) {
    // Shift = range from anchor; Ctrl/Cmd = toggle one into the
    // multi-selection. preventDefault stops the browser from text-selecting
    // row labels while range-clicking.
    if (e.shiftKey) {
      e.preventDefault();
      toggleTripSelection(tripId, {
        range: true,
        order: visibleTrips.map((t) => t.id),
      });
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      toggleTripSelection(tripId);
      return;
    }
    // Plain click: open the trip and collapse any multi-selection, but
    // keep this row as the anchor so a follow-up Shift-click ranges from
    // here.
    useStore.setState({
      selectedTripIds: new Set<string>(),
      tripSelectionAnchorId: tripId,
    });
    void onSelectTrip(tripId);
  }

  async function onConfirmBulkDelete() {
    if (!bulkMode) return;
    setBulkBusy(true);
    setBulkError(null);
    try {
      const ids = bulkTrips.map((t) => t.id);
      const result =
        bulkMode === "trip"
          ? await deleteTripsCompletely(ids)
          : await deleteOriginalsForTrips(ids);
      if (result.failures.length > 0) {
        setBulkError(
          `${result.failures.length} of ${ids.length} ${ids.length === 1 ? "trip" : "trips"} had files that couldn't be moved to trash`,
        );
        // Selection is already cleared by the action; keep the dialog open
        // on the captured snapshot so the failure stays visible.
        return;
      }
      setBulkMode(null);
    } catch (e) {
      setBulkError(e instanceof Error ? e.message : String(e));
    } finally {
      setBulkBusy(false);
    }
  }

  if (!libraryFirstLoadDone) {
    return (
      <div className="flex items-center gap-2 px-3 py-4 text-sm text-neutral-500">
        <span
          className="inline-block h-3 w-3 animate-pulse rounded-full bg-blue-500"
          aria-hidden
        />
        Loading library…
      </div>
    );
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
      {reclaimableFilter && (
        <div className="mx-2 mt-2 flex items-center gap-2 rounded-md border border-emerald-800 bg-emerald-950/60 px-2 py-1.5 text-xs text-emerald-200">
          <span className="flex-1">
            Showing {visibleTrips.length}{" "}
            {visibleTrips.length === 1 ? "trip" : "trips"} with reclaimable
            originals
          </span>
          <button
            onClick={() => setReclaimableFilter(false)}
            className="rounded px-2 py-0.5 text-emerald-300 hover:bg-emerald-900 hover:text-emerald-100"
            title="Show all trips again"
          >
            Clear
          </button>
        </div>
      )}
      {selectedTrips.length > 0 && (
        <div className="mx-2 mt-2 rounded-md border border-sky-700 bg-sky-950/60 px-2 py-1.5 text-xs text-sky-200">
          <div className="flex items-center gap-2">
            <span className="flex-1">
              {selectedTrips.length}{" "}
              {selectedTrips.length === 1 ? "trip" : "trips"} selected
            </span>
            <button
              onClick={() => clearTripSelection()}
              className="rounded px-2 py-0.5 text-sky-300 hover:bg-sky-900 hover:text-sky-100"
              title="Clear the selection"
            >
              Clear
            </button>
          </div>
          <div className="mt-1.5 flex flex-wrap gap-1.5">
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
                  ? "Merge the selected trips into one"
                  : "Select at least one more trip to enable merge"
              }
            >
              Merge
            </button>
            <button
              onClick={() => {
                setBulkTrips(originalsSelectable);
                setBulkError(null);
                setBulkMode("originals");
              }}
              disabled={originalsSelectable.length === 0}
              className={clsx(
                "rounded px-2 py-0.5 font-medium",
                originalsSelectable.length > 0
                  ? "bg-amber-800 text-amber-100 hover:bg-amber-700"
                  : "cursor-not-allowed bg-neutral-800 text-neutral-500",
              )}
              title={
                originalsSelectable.length > 0
                  ? "Trash the source files, keep the timelapse archives"
                  : "No selected trip still has originals to delete"
              }
            >
              Delete originals…
            </button>
            <button
              onClick={() => {
                setBulkTrips(selectedTrips);
                setBulkError(null);
                setBulkMode("trip");
              }}
              className="rounded bg-red-800 px-2 py-0.5 font-medium text-red-100 hover:bg-red-700"
              title="Trash originals and timelapse archives, remove the trips"
            >
              Delete trips…
            </button>
          </div>
        </div>
      )}

      {/* Extra right padding reserves a gutter for the scroll container's
          scrollbar so the per-row kebab (right-1) sits clear of the
          scrollbar track and can't be clicked through it by accident. */}
      <ul className="flex flex-col gap-1 overflow-y-auto py-2 pl-2 pr-4 [scrollbar-gutter:stable]">
        {visibleTrips.map((trip) => {
          const active = trip.id === selectedTripId;
          const archive = trip.archiveOnly === true;
          const selected = selectedTripIds.has(trip.id);
          const archiveBytes = archive
            ? tripTimelapseBytes(trip.id, timelapseJobs)
            : null;
          const originalsBytes = archive ? null : tripOriginalsBytes(trip);
          const timelapseCoverage = archive
            ? null
            : tripTimelapseCoverage(trip.id, timelapseJobs);
          return (
            <li key={trip.id}>
              <div
                className={clsx(
                  "group relative flex items-start rounded-md transition-colors",
                  active
                    ? "bg-neutral-700 text-white"
                    : "text-neutral-300 hover:bg-neutral-800",
                  // Multi-selected trips get a sky-blue ring so the user
                  // can see the group selection at a glance while scrolling.
                  selected && "ring-1 ring-inset ring-sky-500",
                )}
              >
                <button
                  onClick={(e) => onRowClick(e, trip.id)}
                  className="flex-1 select-none px-3 py-2 text-left text-sm"
                >
                  <div className="flex items-center gap-2 pr-7 font-medium">
                    <span>{formatTripStart(trip.startTime)}</span>
                    {archive && (
                      <span
                        className="rounded-sm bg-amber-900/60 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-amber-200"
                        title="Original files have been deleted; only the timelapse archive remains."
                      >
                        Archive
                      </span>
                    )}
                    {timelapseCoverage != null && timelapseCoverage > 0 && (
                      <span
                        className="rounded-sm bg-sky-900/60 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-sky-200"
                        title={`${timelapseCoverage} of ${TOTAL_TIMELAPSE_TUPLES} possible timelapse renders done. Merging this trip with another may delete finished renders if their coverage doesn't match.`}
                      >
                        Timelapse {timelapseCoverage}/{TOTAL_TIMELAPSE_TUPLES}
                      </span>
                    )}
                    {selected && (
                      <span
                        className="rounded-sm bg-sky-900/60 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-sky-200"
                        title="Selected"
                      >
                        ✓
                      </span>
                    )}
                  </div>
                  <div className="text-xs text-neutral-500">
                    {archive
                      ? `Timelapse only · ${formatBytes(archiveBytes)}`
                      : `${trip.segments.length} ${trip.segments.length === 1 ? "segment" : "segments"} · ${formatDuration(trip)} · ${formatBytes(originalsBytes)}`}
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
          marked={selectedTrips}
          onClose={() => setShowMergeDialog(false)}
        />
      )}
      {bulkMode && bulkTrips.length > 0 && (
        <BulkDeleteTripsDialog
          trips={bulkTrips}
          mode={bulkMode}
          busy={bulkBusy}
          errorMessage={bulkError}
          onCancel={() => {
            if (!bulkBusy) setBulkMode(null);
          }}
          onConfirm={() => void onConfirmBulkDelete()}
        />
      )}
    </>
  );
}
