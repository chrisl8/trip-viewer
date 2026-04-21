import { useEffect } from "react";
import clsx from "clsx";
import type { Segment } from "../../types/model";
import { CATEGORY_COLORS } from "../../utils/tagColors";

interface Props {
  segment: Segment;
  hasKeepTag: boolean;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  return (
    d.toLocaleDateString(undefined, { month: "short", day: "numeric" }) +
    " " +
    d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
  );
}

function formatDuration(s: number): string {
  const m = Math.floor(s / 60);
  const sec = Math.round(s % 60);
  return `${m}m ${sec}s`;
}

/**
 * Confirmation dialog for deleting the currently-watched segment.
 * Mirrors the shape of `PlaceDialog` for visual consistency: backdrop
 * click cancels, Esc cancels, centered card. A `keep` warning appears
 * when applicable so the user can't accidentally trash a segment they
 * deliberately preserved.
 */
export function DeleteSegmentDialog({
  segment,
  hasKeepTag,
  busy,
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

  const fileCount = segment.channels.length;

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={onCancel}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-96 rounded-md border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
      >
        <h2 className="text-base font-semibold">Delete this segment?</h2>
        <p className="mt-2 text-sm text-neutral-400">
          {formatTime(segment.startTime)} ·{" "}
          {formatDuration(segment.durationS)}
        </p>
        <p className="mt-1 text-xs text-neutral-500">
          {fileCount} {fileCount === 1 ? "channel file" : "channel files"} will
          move to the OS trash. Recoverable from there.
        </p>
        {hasKeepTag && (
          <p className="mt-2 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
            This segment is marked{" "}
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
            disabled={busy}
            className={clsx(
              "rounded-md px-3 py-1 text-sm text-white",
              busy
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
