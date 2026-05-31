import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import type { Trip } from "../../types/model";
import { formatBytes } from "../../utils/format";
import { type ArchiveOnDisk, tripArchiveOnDisk } from "../../ipc/timelapse";
import { computeTripArchiveStatus } from "./tripArchiveStatus";

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

/** Sum segment sizes — null if any are unknown (mixed totals would mislead). */
function trashedBytes(trip: Trip): number | null {
  if (trip.segments.length === 0) return null;
  let total = 0;
  for (const seg of trip.segments) {
    if (seg.sizeBytes == null) return null;
    total += seg.sizeBytes;
  }
  return total;
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
  const originalsBytes = useMemo(() => trashedBytes(trip), [trip]);
  const jobs = useStore((s) => s.timelapseJobs);
  const { archiveExists, archiveBytes } = useMemo(
    () => computeTripArchiveStatus(jobs, trip.id),
    [jobs, trip.id],
  );

  // Live on-disk verification. The DB `status='done'` can lie: the file
  // may have been deleted out from under us, or sit on an unplugged
  // drive. Block the irreversible delete on the insidious case — an
  // archive that *claims* to exist but whose files are gone — without
  // touching the legitimate "no archive, discard this footage" choice.
  const [onDisk, setOnDisk] = useState<ArchiveOnDisk | null>(null);
  const [checking, setChecking] = useState(true);
  useEffect(() => {
    let active = true;
    setChecking(true);
    tripArchiveOnDisk(trip.id)
      .then((r) => active && setOnDisk(r))
      .catch(() => active && setOnDisk(null))
      .finally(() => active && setChecking(false));
    return () => {
      active = false;
    };
  }, [trip.id]);

  const blockReason = checking
    ? "Verifying the timelapse archive on disk…"
    : !onDisk
      ? "Couldn't verify the timelapse archive — try again."
      : !onDisk.archiveReachable && onDisk.doneJobs > 0
        ? "Can't reach the archive drive. Connect it so the timelapse can be verified before deleting originals."
        : onDisk.missingFiles.length > 0
          ? `${onDisk.missingFiles.length} timelapse file(s) for this trip are missing on disk — the archive is incomplete. Rebuild this trip before deleting its originals.`
          : null;
  const blocked = blockReason !== null;

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
          {trip.segments.length} {trip.segments.length === 1 ? "segment" : "segments"} · {formatDuration(totalDuration)}
        </p>
        <p className="mt-3 text-sm text-neutral-300">
          {fileCount} original {fileCount === 1 ? "file" : "files"}
          {originalsBytes != null && ` (${formatBytes(originalsBytes)})`} will
          be moved to the OS trash. Recoverable from there.
        </p>
        {archiveExists ? (
          <p className="mt-2 rounded-md bg-emerald-950 px-2 py-1 text-xs text-emerald-300">
            The timelapse archive
            {archiveBytes != null && ` (${formatBytes(archiveBytes)})`} will
            be kept and stays playable in this trip.
          </p>
        ) : (
          <p className="mt-2 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
            This trip has no timelapse archive. Once the OS trash is emptied,
            this footage won't be recoverable.
          </p>
        )}
        {/* Hard block — only for an archive that should exist but whose
            files are missing/unverifiable. Distinct (red) from the soft
            amber "no archive" choice above. */}
        {blocked && archiveExists && (
          <p className="mt-2 rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
            {blockReason}
          </p>
        )}
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
            disabled={busy || (blocked && archiveExists)}
            className={clsx(
              "rounded-md px-3 py-1 text-sm text-white",
              busy || (blocked && archiveExists)
                ? "cursor-not-allowed bg-neutral-700"
                : "bg-red-700 hover:bg-red-600",
            )}
          >
            {busy
              ? "Deleting…"
              : checking && archiveExists
                ? "Verifying…"
                : "Move originals to trash"}
          </button>
        </div>
      </div>
    </div>
  );
}
