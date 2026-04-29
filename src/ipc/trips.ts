import { invoke } from "@tauri-apps/api/core";
import type { Trip } from "../types/model";
import type { DeleteFailure } from "./tags";

/**
 * Trips that exist in the DB only because they have a timelapse archive
 * — their source segments have all been deleted. Returned with
 * `segments: []` and `archiveOnly: true` so the frontend can interleave
 * them into the main trip list and filter for them in the UI without a
 * separate code path.
 */
export function listArchiveOnlyTrips(): Promise<Trip[]> {
  return invoke<Trip[]>("list_archive_only_trips");
}

export interface DeleteTripReport {
  segmentFilesTrashed: number;
  timelapseFilesTrashed: number;
  timelapseJobsRemoved: number;
  tripRemoved: boolean;
  failures: DeleteFailure[];
}

/**
 * Wholesale "delete this entire trip". Trashes every source MP4 *and*
 * the trip's timelapse pre-renders, then removes all DB rows. This is
 * the only IPC that ever removes a timelapse archive — per-segment
 * deletion never touches it.
 *
 * `inMemoryPaths` maps segmentId → channel paths so the backend can
 * trash every channel file (DB only stores the master path).
 */
export function deleteTrip(
  tripId: string,
  inMemoryPaths: Record<string, string[]>,
): Promise<DeleteTripReport> {
  return invoke<DeleteTripReport>("delete_trip", {
    tripId,
    inMemoryPaths,
  });
}

// ── Manual trip merge ──────────────────────────────────────────────

export type TupleStatus = "concatenable" | "partialOutputs";

export interface TupleAssessment {
  tier: string;
  channel: string;
  status: TupleStatus;
  primaryHas: boolean;
  absorbedWithOutput: string[];
}

export interface TimelapseMergeAssessment {
  /** False when no source trip has any timelapse_jobs row — frontend
   *  can skip the strategy dialog and merge silently. */
  hasAnyTimelapses: boolean;
  tuples: TupleAssessment[];
}

export type TimelapseMergeStrategy = "concatWherePossible" | "discardAll";

export interface MergeReport {
  primaryTripId: string;
  absorbedTripIds: string[];
  /** Tuples successfully concatenated (one entry per [tier, channel]). */
  concatenated: [string, string][];
  /** Total timelapse_jobs rows removed. */
  timelapseJobsRemoved: number;
}

export function assessTripMerge(
  primaryTripId: string,
  absorbedTripIds: string[],
): Promise<TimelapseMergeAssessment> {
  return invoke<TimelapseMergeAssessment>("assess_trip_merge", {
    primaryTripId,
    absorbedTripIds,
  });
}

export function mergeTrips(
  primaryTripId: string,
  absorbedTripIds: string[],
  strategy: TimelapseMergeStrategy,
): Promise<MergeReport> {
  return invoke<MergeReport>("merge_trips", {
    primaryTripId,
    absorbedTripIds,
    strategy,
  });
}
