import { useEffect, useMemo } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import { CATEGORY_COLORS } from "../../utils/tagColors";
import { formatBytes } from "../../utils/format";

interface Props {
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

function formatDuration(s: number): string {
  if (s < 60) return `${Math.round(s)}s`;
  const m = Math.floor(s / 60);
  const sec = Math.round(s % 60);
  if (m < 60) return sec === 0 ? `${m}m` : `${m}m ${sec}s`;
  const h = Math.floor(m / 60);
  const remM = m % 60;
  return remM === 0 ? `${h}h` : `${h}h ${remM}m`;
}

/**
 * Bulk-delete confirmation. Shows the selection's count, total
 * duration, and a `keep`-tag warning if any selected segments are
 * marked keep — same review-state hint as single-segment delete.
 */
export function DeleteSelectedSegmentsDialog({
  busy,
  onCancel,
  onConfirm,
}: Props) {
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const selectedSegmentIds = useStore((s) => s.selectedSegmentIds);
  const tagsBySegmentId = useStore((s) => s.tagsBySegmentId);

  const summary = useMemo(() => {
    const trip = trips.find((t) => t.id === loadedTripId);
    if (!trip) {
      return { count: 0, totalDuration: 0, fileCount: 0, keptCount: 0 };
    }
    let count = 0;
    let totalDuration = 0;
    let fileCount = 0;
    let keptCount = 0;
    for (const seg of trip.segments) {
      if (!selectedSegmentIds.has(seg.id)) continue;
      count += 1;
      totalDuration += seg.durationS;
      fileCount += seg.channels.length;
      const tags = tagsBySegmentId[seg.id] ?? [];
      if (tags.some((t) => t.name === "keep")) keptCount += 1;
    }
    return { count, totalDuration, fileCount, keptCount };
  }, [trips, loadedTripId, selectedSegmentIds, tagsBySegmentId]);

  const archiveBytes = useStore((s) => {
    const tripJobs = s.timelapseJobs.filter(
      (j) => j.tripId === loadedTripId && j.outputSizeBytes != null,
    );
    if (tripJobs.length === 0) return null;
    return tripJobs.reduce((sum, j) => sum + (j.outputSizeBytes ?? 0), 0);
  });

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onCancel();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={onCancel}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-96 rounded-md border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
      >
        <h2 className="text-base font-semibold">
          Delete {summary.count}{" "}
          {summary.count === 1 ? "segment" : "segments"}?
        </h2>
        <p className="mt-2 text-sm text-neutral-400">
          {formatDuration(summary.totalDuration)} of footage ·{" "}
          {summary.fileCount}{" "}
          {summary.fileCount === 1 ? "channel file" : "channel files"} will
          move to the OS trash. Recoverable from there.
        </p>
        {archiveBytes != null ? (
          <p className="mt-2 rounded-md bg-emerald-950 px-2 py-1 text-xs text-emerald-300">
            This trip's timelapse archive ({formatBytes(archiveBytes)}) is
            kept and stays playable.
          </p>
        ) : (
          <p className="mt-2 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
            This trip has no timelapse archive. Once the OS trash is emptied,
            this footage won't be recoverable.
          </p>
        )}
        {summary.keptCount > 0 && (
          <p className="mt-2 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
            {summary.keptCount} of these{" "}
            {summary.keptCount === 1 ? "is" : "are"} marked{" "}
            <span className={CATEGORY_COLORS.user.text}>keep</span>. Delete
            anyway?
          </p>
        )}
        <div className="mt-4 flex justify-end gap-2">
          <button
            onClick={onCancel}
            disabled={busy}
            className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={busy || summary.count === 0}
            className={clsx(
              "rounded-md px-3 py-1 text-sm text-white",
              busy || summary.count === 0
                ? "cursor-not-allowed bg-neutral-700"
                : "bg-red-700 hover:bg-red-600",
            )}
          >
            Move to trash
          </button>
        </div>
      </div>
    </div>
  );
}
