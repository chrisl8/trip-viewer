import { useEffect, useMemo } from "react";
import clsx from "clsx";
import type { Trip } from "../../types/model";

interface Props {
  trip: Trip;
  busy: boolean;
  errorMessage: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

function formatDuration(s: number): string {
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

/**
 * Confirms the "delete originals" action: trashes every source MP4
 * for a trip while leaving the timelapse archive intact. Distinct copy
 * from `DeleteTripDialog` so the user can't confuse the two — this is
 * the disk-reclaim step in the timelapse-as-archive workflow.
 */
export function DeleteOriginalsDialog({
  trip,
  busy,
  errorMessage,
  onCancel,
  onConfirm,
}: Props) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onCancel();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const fileCount = useMemo(
    () =>
      trip.segments.reduce((sum, seg) => sum + seg.channels.length, 0),
    [trip.segments],
  );
  const totalDuration = useMemo(
    () => trip.segments.reduce((sum, seg) => sum + seg.durationS, 0),
    [trip.segments],
  );

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={onCancel}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-[28rem] rounded-md border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
      >
        <h2 className="text-base font-semibold">Delete original files?</h2>
        <p className="mt-2 text-sm text-neutral-400">
          {trip.segments.length} segments · {formatDuration(totalDuration)}
        </p>
        <p className="mt-3 text-sm text-neutral-300">
          {fileCount} source {fileCount === 1 ? "file" : "files"} will be moved
          to the OS trash. Recoverable from there.
        </p>
        <p className="mt-2 rounded-md bg-emerald-950 px-2 py-1 text-xs text-emerald-300">
          The timelapse archive will be kept and stays playable in this trip.
        </p>
        {errorMessage && (
          <p className="mt-2 rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
            {errorMessage}
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
            disabled={busy}
            className={clsx(
              "rounded-md px-3 py-1 text-sm text-white",
              busy
                ? "cursor-not-allowed bg-neutral-700"
                : "bg-red-700 hover:bg-red-600",
            )}
          >
            {busy ? "Deleting…" : "Move originals to trash"}
          </button>
        </div>
      </div>
    </div>
  );
}
