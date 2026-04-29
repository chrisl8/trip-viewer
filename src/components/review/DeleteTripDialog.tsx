import { useEffect, useMemo } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";

interface Props {
  trip: Trip;
  busy: boolean;
  errorMessage: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

function formatDuration(s: number): string {
  if (s <= 0) return "0m";
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

/**
 * Confirms the "delete trip" action: trashes every source file *and*
 * every timelapse pre-render, then removes the trip from the library
 * entirely. The only path that ever removes a timelapse archive — copy
 * emphasizes that explicitly so it's not confused with `Delete originals…`.
 */
export function DeleteTripDialog({
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

  const segmentFileCount = useMemo(
    () =>
      trip.segments.reduce((sum, seg) => sum + seg.channels.length, 0),
    [trip.segments],
  );
  const totalDuration = useMemo(
    () => trip.segments.reduce((sum, seg) => sum + seg.durationS, 0),
    [trip.segments],
  );
  const timelapseFileCount = useStore((s) =>
    s.timelapseJobs.filter(
      (j) => j.tripId === trip.id && j.outputPath !== null,
    ).length,
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
        <h2 className="text-base font-semibold text-red-300">
          Delete entire trip?
        </h2>
        {trip.segments.length > 0 ? (
          <p className="mt-2 text-sm text-neutral-400">
            {trip.segments.length} {trip.segments.length === 1 ? "segment" : "segments"} · {formatDuration(totalDuration)}
          </p>
        ) : (
          <p className="mt-2 text-sm text-neutral-400">Archive only</p>
        )}
        <ul className="mt-3 space-y-1 text-sm text-neutral-300">
          {segmentFileCount > 0 && (
            <li>
              <span className="text-neutral-400">→</span> {segmentFileCount}{" "}
              source {segmentFileCount === 1 ? "file" : "files"} to trash
            </li>
          )}
          {timelapseFileCount > 0 && (
            <li>
              <span className="text-neutral-400">→</span> {timelapseFileCount}{" "}
              timelapse archive{" "}
              {timelapseFileCount === 1 ? "file" : "files"} to trash
            </li>
          )}
          <li>
            <span className="text-neutral-400">→</span> Trip and all its tags
            removed from the library
          </li>
        </ul>
        <p className="mt-2 rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
          This deletes the timelapse archive too. Use{" "}
          <span className="font-semibold">Delete originals…</span> instead if
          you want to keep the archive.
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
            {busy ? "Deleting…" : "Delete trip"}
          </button>
        </div>
      </div>
    </div>
  );
}
