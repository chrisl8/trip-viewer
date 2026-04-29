import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";
import { DeleteOriginalsDialog } from "../review/DeleteOriginalsDialog";
import { DeleteTripDialog } from "../review/DeleteTripDialog";

interface Props {
  trip: Trip;
  /** Visual style. "kebab" renders a vertical-dots button (sidebar);
   *  "icon" renders a slightly bigger square (table cells). */
  variant?: "kebab" | "icon";
}

/**
 * Per-trip kebab menu surfacing the two trip-level delete actions:
 * "Delete originals…" (trash sources, keep timelapse archive) and
 * "Delete trip…" (trash everything). Hides "Delete originals…" when
 * the trip is already archive-only since there's nothing to delete.
 */
export function TripActionsMenu({ trip, variant = "kebab" }: Props) {
  const [open, setOpen] = useState(false);
  const [showDeleteOriginals, setShowDeleteOriginals] = useState(false);
  const [showDeleteTrip, setShowDeleteTrip] = useState(false);
  const [busy, setBusy] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const deleteOriginalsForTrip = useStore((s) => s.deleteOriginalsForTrip);
  const deleteTripCompletely = useStore((s) => s.deleteTripCompletely);
  const markedForMerge = useStore((s) => s.markedForMerge);
  const toggleMarkForMerge = useStore((s) => s.toggleMarkForMerge);
  const isMarked = markedForMerge.has(trip.id);

  // Close popover on outside click. Pointerdown so it races ahead of
  // re-renders triggered by inner clicks.
  useEffect(() => {
    if (!open) return;
    function onPointer(e: PointerEvent) {
      if (!menuRef.current) return;
      if (!(e.target instanceof Node)) return;
      if (menuRef.current.contains(e.target)) return;
      setOpen(false);
    }
    document.addEventListener("pointerdown", onPointer);
    return () => document.removeEventListener("pointerdown", onPointer);
  }, [open]);

  const archive = trip.archiveOnly === true;

  async function onConfirmDeleteOriginals() {
    setBusy(true);
    setErrorMessage(null);
    try {
      const report = await deleteOriginalsForTrip(trip.id);
      if (report.failures.length > 0) {
        setErrorMessage(
          report.failures.length === 1
            ? `Failed: ${report.failures[0].message}`
            : `${report.failures.length} files could not be moved to trash`,
        );
        // Don't auto-close on partial failure — user needs to see what
        // happened. They can dismiss with Cancel.
        return;
      }
      setShowDeleteOriginals(false);
    } catch (e) {
      setErrorMessage(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onConfirmDeleteTrip() {
    setBusy(true);
    setErrorMessage(null);
    try {
      const report = await deleteTripCompletely(trip.id);
      if (report.failures.length > 0) {
        setErrorMessage(
          report.failures.length === 1
            ? `Failed: ${report.failures[0].message}`
            : `${report.failures.length} files could not be moved to trash`,
        );
        // Trip rows are removed even on partial file-trash failure
        // (the store action only removes them when tripRemoved=true,
        // which the backend sets after the DB transaction commits).
        // So the dialog stays open to surface the failure list, but
        // the trip is already gone from the sidebar.
        return;
      }
      setShowDeleteTrip(false);
    } catch (e) {
      setErrorMessage(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div ref={menuRef} className="relative inline-block">
        <button
          onClick={(e) => {
            e.stopPropagation();
            setOpen((o) => !o);
          }}
          className={clsx(
            "flex items-center justify-center rounded text-neutral-400 hover:bg-neutral-700 hover:text-neutral-100",
            variant === "kebab" ? "h-6 w-6 text-base" : "h-7 w-7 text-base",
          )}
          aria-label="Trip actions"
          title="Trip actions"
        >
          ⋮
        </button>
        {open && (
          <div
            className="absolute right-0 top-full z-20 mt-1 min-w-[12rem] rounded-md border border-neutral-700 bg-neutral-900 py-1 shadow-lg"
            onClick={(e) => e.stopPropagation()}
          >
            <button
              onClick={() => {
                setOpen(false);
                toggleMarkForMerge(trip.id);
              }}
              className="block w-full border-b border-neutral-800 px-3 py-1.5 text-left text-sm text-neutral-200 hover:bg-neutral-800"
            >
              {isMarked ? "Unmark for merge" : "Mark for merge"}
              <div className="text-[11px] text-neutral-500">
                {isMarked
                  ? "Remove from the merge selection"
                  : "Mark this trip; mark another to enable join"}
              </div>
            </button>
            {!archive && (
              <button
                onClick={() => {
                  setOpen(false);
                  setErrorMessage(null);
                  setShowDeleteOriginals(true);
                }}
                className="block w-full px-3 py-1.5 text-left text-sm text-neutral-200 hover:bg-neutral-800"
              >
                Delete originals…
                <div className="text-[11px] text-neutral-500">
                  Keep the timelapse archive
                </div>
              </button>
            )}
            <button
              onClick={() => {
                setOpen(false);
                setErrorMessage(null);
                setShowDeleteTrip(true);
              }}
              className="block w-full px-3 py-1.5 text-left text-sm text-red-300 hover:bg-neutral-800"
            >
              Delete trip…
              <div className="text-[11px] text-neutral-500">
                Sources and timelapse archive
              </div>
            </button>
          </div>
        )}
      </div>

      {showDeleteOriginals && (
        <DeleteOriginalsDialog
          trip={trip}
          busy={busy}
          errorMessage={errorMessage}
          onCancel={() => {
            if (!busy) setShowDeleteOriginals(false);
          }}
          onConfirm={() => void onConfirmDeleteOriginals()}
        />
      )}
      {showDeleteTrip && (
        <DeleteTripDialog
          trip={trip}
          busy={busy}
          errorMessage={errorMessage}
          onCancel={() => {
            if (!busy) setShowDeleteTrip(false);
          }}
          onConfirm={() => void onConfirmDeleteTrip()}
        />
      )}
    </>
  );
}
