import { useEffect, useState } from "react";
import clsx from "clsx";
import {
  addUserTag,
  deleteSegmentsToTrash,
  removeUserTag,
} from "../../ipc/tags";
import { useStore } from "../../state/store";
import type { Segment, Tag } from "../../types/model";
import { CATEGORY_COLORS } from "../../utils/tagColors";
import { PlaceDialog } from "../places/PlaceDialog";
import { DeleteSegmentDialog } from "./DeleteSegmentDialog";
import { DeleteSelectedSegmentsDialog } from "./DeleteSelectedSegmentsDialog";
import { TagBadge } from "./TagBadge";

interface Props {
  segment: Segment;
}

/**
 * Stable reference for the "no tags yet" case. Must NOT inline `?? []`
 * in the selector — that returns a fresh array per call and zustand's
 * `Object.is` snapshot check then sees a new value every render,
 * triggering an infinite update loop.
 */
const EMPTY_TAGS: Tag[] = [];

/**
 * Thin strip above the timeline showing the current segment's tags as
 * colored pills, plus a row of outlined toggle pills for every
 * developer-curated user-applicable tag (e.g. `parked`, `keep`). Click
 * a toggle pill to add/remove that tag on the current segment.
 */
export function SegmentTagBar({ segment }: Props) {
  const tags = useStore((s) => s.tagsBySegmentId[segment.id] ?? EMPTY_TAGS);
  const userApplicable = useStore((s) => s.userApplicableTags);
  const refreshTripTags = useStore((s) => s.refreshTripTags);
  const selectedTripId = useStore((s) => s.selectedTripId);
  const refreshTripTagCounts = useStore((s) => s.refreshTripTagCounts);
  const refreshPlaces = useStore((s) => s.refreshPlaces);
  const removeSegmentFromTrip = useStore((s) => s.removeSegmentFromTrip);
  const removeSegmentsFromTrip = useStore((s) => s.removeSegmentsFromTrip);
  const setActiveSegmentId = useStore((s) => s.setActiveSegmentId);
  const setIsPlaying = useStore((s) => s.setIsPlaying);
  const gpsByFile = useStore((s) => s.gpsByFile);
  const selectionMode = useStore((s) => s.selectionMode);
  const selectedSegmentIds = useStore((s) => s.selectedSegmentIds);
  const enterSelectionMode = useStore((s) => s.enterSelectionMode);
  const exitSelectionMode = useStore((s) => s.exitSelectionMode);
  const [busyName, setBusyName] = useState<string | null>(null);
  const [placeDialogGps, setPlaceDialogGps] = useState<
    { lat: number; lon: number } | "empty" | null
  >(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmBulkDelete, setConfirmBulkDelete] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // Esc cancels selection mode (only if no dialog is open, since the
  // dialog has its own Esc handler that should win).
  useEffect(() => {
    if (!selectionMode) return;
    if (confirmBulkDelete) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") exitSelectionMode();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selectionMode, confirmBulkDelete, exitSelectionMode]);

  function onSaveAsPlace() {
    const path = segment.channels[0]?.filePath;
    const points = path ? gpsByFile[path] ?? [] : [];
    const locked = points.filter((p) => p.fixOk);
    if (locked.length === 0) {
      // Open the dialog empty so the user can type lat/lon manually.
      setPlaceDialogGps("empty");
      return;
    }
    // Mean of the locked points. For stationary/parked segments every
    // point is at the same spot, so mean == that spot. For moving
    // segments the user shouldn't really be saving a place here.
    let sumLat = 0;
    let sumLon = 0;
    for (const p of locked) {
      sumLat += p.lat;
      sumLon += p.lon;
    }
    setPlaceDialogGps({
      lat: sumLat / locked.length,
      lon: sumLon / locked.length,
    });
  }

  async function onConfirmDelete() {
    if (!selectedTripId) return;
    // Read current trip from store at the moment of click — `segment`
    // is a stale prop after the splice fires, but we need the index now
    // before we mutate.
    const trip = useStore
      .getState()
      .trips.find((t) => t.id === selectedTripId);
    if (!trip) return;

    const idx = trip.segments.findIndex((s) => s.id === segment.id);
    const nextId = idx >= 0 ? trip.segments[idx + 1]?.id ?? null : null;
    const channelPaths = segment.channels
      .map((c) => c.filePath)
      .filter(Boolean);

    setDeleteBusy(true);
    setDeleteError(null);
    try {
      const report = await deleteSegmentsToTrash(
        [segment.id],
        { [segment.id]: channelPaths },
      );
      if (report.failures.length > 0) {
        // Backend leaves the segment row intact when nothing trashed,
        // so don't splice — just surface the error and let the user
        // retry or move on.
        setDeleteError(
          report.failures.length === 1
            ? `Failed: ${report.failures[0].message}`
            : `${report.failures.length} files failed to delete`,
        );
        setDeleteBusy(false);
        setConfirmDelete(false);
        return;
      }

      removeSegmentFromTrip(selectedTripId, segment.id);
      if (nextId) {
        setActiveSegmentId(nextId);
      } else {
        // Last segment in the trip — stop playback. If the trip itself
        // is now empty, removeSegmentFromTrip already advanced
        // selectedTripId; the user lands on the next trip's player or
        // an empty state.
        setIsPlaying(false);
        setActiveSegmentId(null);
      }
      await refreshTripTagCounts();
      if (selectedTripId) {
        await refreshTripTags(selectedTripId);
      }
    } catch (e) {
      console.error("delete segment failed", e);
      setDeleteError(String(e));
    } finally {
      setDeleteBusy(false);
      setConfirmDelete(false);
    }
  }

  async function onConfirmBulkDelete() {
    if (!selectedTripId) return;
    const trip = useStore
      .getState()
      .trips.find((t) => t.id === selectedTripId);
    if (!trip) return;

    // Snapshot selection at click time (the store may mutate as
    // segments get spliced out below).
    const idsToDelete = Array.from(selectedSegmentIds);
    if (idsToDelete.length === 0) return;

    // Build the channel-paths map from the in-memory trip so the
    // backend doesn't have to look up channel lists.
    const idSet = new Set(idsToDelete);
    const inMemoryPaths: Record<string, string[]> = {};
    for (const seg of trip.segments) {
      if (!idSet.has(seg.id)) continue;
      inMemoryPaths[seg.id] = seg.channels
        .map((c) => c.filePath)
        .filter(Boolean);
    }

    // Pick a sensible next active segment: the first segment in the
    // current trip that *isn't* being deleted, scanning forward from
    // the last deleted index (so playback continues past the run).
    const deletedIndices = trip.segments
      .map((s, i) => (idSet.has(s.id) ? i : -1))
      .filter((i) => i >= 0);
    const lastDeletedIdx = deletedIndices[deletedIndices.length - 1] ?? -1;
    let nextId: string | null = null;
    for (let i = lastDeletedIdx + 1; i < trip.segments.length; i++) {
      if (!idSet.has(trip.segments[i].id)) {
        nextId = trip.segments[i].id;
        break;
      }
    }
    if (!nextId) {
      // No segment after the deleted run — try the one immediately
      // before. (Don't restart the trip from the top: that's surprising.)
      for (let i = deletedIndices[0] - 1; i >= 0; i--) {
        if (!idSet.has(trip.segments[i].id)) {
          nextId = trip.segments[i].id;
          break;
        }
      }
    }

    setDeleteBusy(true);
    setDeleteError(null);
    try {
      const report = await deleteSegmentsToTrash(idsToDelete, inMemoryPaths);
      // Splice every segment the backend successfully removed. The
      // backend returns segmentsRemoved as a count, not a list, so use
      // the per-file failure list as the inverse: anything still
      // referenced by a failure stays put. Since failures are reported
      // at file granularity but our backend also reports any-success
      // per segment internally, we approximate: if NO failures, every
      // selected segment is gone.
      if (report.failures.length === 0) {
        removeSegmentsFromTrip(selectedTripId, idsToDelete);
      } else {
        // Conservative: figure out which segments lost all their files
        // by checking which paths failed. Any segment with any failure
        // path stays in the list.
        const failedPaths = new Set(report.failures.map((f) => f.path));
        const survivors = new Set<string>();
        for (const segId of idsToDelete) {
          const paths = inMemoryPaths[segId] ?? [];
          if (paths.some((p) => failedPaths.has(p))) {
            survivors.add(segId);
          }
        }
        const removed = idsToDelete.filter((id) => !survivors.has(id));
        if (removed.length > 0) {
          removeSegmentsFromTrip(selectedTripId, removed);
        }
        setDeleteError(
          `${report.failures.length} file${report.failures.length === 1 ? "" : "s"} couldn't be moved to trash`,
        );
      }

      // After mutation, the active segment might be one of the deleted
      // ones. Advance to the captured next id (or null if the trip was
      // wiped).
      if (nextId) {
        setActiveSegmentId(nextId);
      } else {
        setIsPlaying(false);
        setActiveSegmentId(null);
      }
      exitSelectionMode();
      await refreshTripTagCounts();
      if (selectedTripId) await refreshTripTags(selectedTripId);
    } catch (e) {
      console.error("bulk delete failed", e);
      setDeleteError(String(e));
    } finally {
      setDeleteBusy(false);
      setConfirmBulkDelete(false);
    }
  }

  async function toggleTag(name: string) {
    const currentlyHas = tags.some((t) => t.name === name);
    setBusyName(name);
    try {
      if (currentlyHas) {
        await removeUserTag([segment.id], name);
      } else {
        await addUserTag([segment.id], name);
      }
      if (selectedTripId) await refreshTripTags(selectedTripId);
      await refreshTripTagCounts();
    } catch (e) {
      console.error(`toggleTag(${name}) failed`, e);
    } finally {
      setBusyName(null);
    }
  }

  if (selectionMode) {
    const selectedCount = selectedSegmentIds.size;
    return (
      <>
        <div className="flex items-center gap-3 border-y border-rose-900/60 bg-rose-950/30 px-4 py-1.5 text-sm">
          <span className="font-medium text-rose-200">Selection mode</span>
          <span className="text-xs text-rose-300/80">
            Click segments on the timeline to add or remove. Shift-click for
            ranges. Esc to exit.
          </span>
          <span className="ml-auto text-rose-200">
            {selectedCount}{" "}
            {selectedCount === 1 ? "segment" : "segments"} selected
          </span>
          <button
            onClick={() => setConfirmBulkDelete(true)}
            disabled={selectedCount === 0}
            className={clsx(
              "rounded px-2 py-0.5 text-xs",
              selectedCount === 0
                ? "cursor-not-allowed border border-neutral-700 text-neutral-500"
                : "bg-red-700 text-white hover:bg-red-600",
            )}
          >
            Delete selected…
          </button>
          <button
            onClick={() => exitSelectionMode()}
            className="rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-300 hover:bg-neutral-800"
          >
            Cancel
          </button>
        </div>
        {confirmBulkDelete && (
          <DeleteSelectedSegmentsDialog
            busy={deleteBusy}
            onCancel={() => setConfirmBulkDelete(false)}
            onConfirm={() => void onConfirmBulkDelete()}
          />
        )}
        {deleteError && (
          <div className="fixed bottom-4 right-4 z-30 max-w-sm rounded-md border border-red-700 bg-neutral-900 p-3 text-sm shadow-lg">
            <div className="flex items-start justify-between gap-2">
              <div>
                <div className="font-medium text-red-300">
                  Some files weren't deleted
                </div>
                <div className="mt-1 text-xs text-neutral-400">
                  {deleteError}
                </div>
              </div>
              <button
                onClick={() => setDeleteError(null)}
                className="shrink-0 text-neutral-400 hover:text-neutral-200"
              >
                ×
              </button>
            </div>
          </div>
        )}
      </>
    );
  }

  return (
    <div className="flex items-center gap-2 px-4 py-1">
      {tags.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {tags.map((tag) => (
            <TagBadge
              key={tag.id ?? `${tag.name}-${tag.source}`}
              tag={tag}
              compact
            />
          ))}
        </div>
      )}
      <div className="ml-auto flex flex-wrap items-center gap-1">
        {segment.gpsSupported && (
          <button
            onClick={onSaveAsPlace}
            className="rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-400 transition-colors hover:border-rose-500 hover:text-rose-300"
            title="Save this segment's GPS location as a named place"
          >
            Save as place…
          </button>
        )}
        {userApplicable.map((ut) => {
          const active = tags.some((t) => t.name === ut.name);
          const colors = CATEGORY_COLORS[ut.category];
          const busy = busyName === ut.name;
          return (
            <button
              key={ut.name}
              onClick={() => void toggleTag(ut.name)}
              disabled={busy}
              title={ut.description}
              className={clsx(
                "rounded border px-2 py-0.5 text-xs transition-colors",
                active
                  ? clsx(colors.bg, colors.text, colors.border)
                  : clsx(
                      "border-neutral-700 text-neutral-400",
                      colors.hoverBorder,
                      colors.hoverText,
                    ),
                busy && "opacity-50",
              )}
            >
              {active ? "✓ " : ""}
              {ut.displayName}
            </button>
          );
        })}
        <button
          onClick={() => enterSelectionMode()}
          className="rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-400 transition-colors hover:border-rose-500 hover:text-rose-300"
          title="Select multiple segments to delete in bulk"
        >
          Select segments…
        </button>
        <button
          onClick={() => setConfirmDelete(true)}
          className="rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-400 transition-colors hover:border-red-500 hover:text-red-300"
          title="Delete this segment (move all channel files to OS trash)"
        >
          Delete segment…
        </button>
      </div>
      {placeDialogGps !== null && (
        <PlaceDialog
          initialLat={
            placeDialogGps !== "empty" ? placeDialogGps.lat : undefined
          }
          initialLon={
            placeDialogGps !== "empty" ? placeDialogGps.lon : undefined
          }
          onClose={() => setPlaceDialogGps(null)}
          onSaved={() => void refreshPlaces()}
        />
      )}
      {confirmDelete && (
        <DeleteSegmentDialog
          segment={segment}
          hasKeepTag={tags.some((t) => t.name === "keep")}
          busy={deleteBusy}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void onConfirmDelete()}
        />
      )}
      {deleteError && (
        <div className="fixed bottom-4 right-4 z-30 max-w-sm rounded-md border border-red-700 bg-neutral-900 p-3 text-sm shadow-lg">
          <div className="flex items-start justify-between gap-2">
            <div>
              <div className="font-medium text-red-300">
                Couldn't delete segment
              </div>
              <div className="mt-1 text-xs text-neutral-400">
                {deleteError}
              </div>
            </div>
            <button
              onClick={() => setDeleteError(null)}
              className="shrink-0 text-neutral-400 hover:text-neutral-200"
            >
              ×
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
