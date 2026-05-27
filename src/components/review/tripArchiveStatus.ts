/**
 * Decide whether a trip's timelapse archive exists and (if so) how
 * many bytes it occupies on disk.
 *
 * These are two distinct questions. An earlier version of the
 * Delete-Originals / Delete-Selected-Segments dialogs filtered to
 * `outputSizeBytes != null` for BOTH purposes, which misreported
 * merged-trip rows as having no archive: the trip merge code
 * persisted `status='done'` rows without `output_size_bytes` (the
 * `cleanup::backfill_output_sizes` startup pass fills them in later,
 * but until that runs the column is NULL). The dialog then warned
 * the user that footage wasn't recoverable when in fact a usable
 * timelapse was sitting on disk.
 *
 * Existence is determined by `status='done'` alone — that's the
 * source of truth for "we have a usable timelapse file." Total bytes
 * is reported only when every done row has a populated size, so we
 * never display a misleading partial total.
 */
export interface JobRowForArchive {
  tripId: string;
  status: string;
  outputSizeBytes: number | null;
}

export interface TripArchiveStatus {
  /** True if at least one done timelapse_jobs row exists for the trip. */
  archiveExists: boolean;
  /** Total bytes across all done rows, or `null` when at least one
   *  done row has an unknown size. */
  archiveBytes: number | null;
}

export function computeTripArchiveStatus(
  jobs: readonly JobRowForArchive[],
  tripId: string | null,
): TripArchiveStatus {
  if (tripId == null) {
    return { archiveExists: false, archiveBytes: null };
  }
  const doneJobs = jobs.filter(
    (j) => j.tripId === tripId && j.status === "done",
  );
  if (doneJobs.length === 0) {
    return { archiveExists: false, archiveBytes: null };
  }
  const haveAllSizes = doneJobs.every((j) => j.outputSizeBytes != null);
  const sum = doneJobs.reduce((acc, j) => acc + (j.outputSizeBytes ?? 0), 0);
  return {
    archiveExists: true,
    archiveBytes: haveAllSizes ? sum : null,
  };
}
