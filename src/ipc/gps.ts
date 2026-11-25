import { Channel, invoke } from "@tauri-apps/api/core";
import type { CameraKind, GpsBatchItem, GpsPoint, Trip } from "../types/model";

/**
 * A GPS extraction request. The backend dispatches to a brand-specific
 * decoder based on `cameraKind` because each dashcam stores GPS in its own
 * proprietary layout (ShenShu meta-track for Wolf Box, gps0 atom for
 * Miltona, nothing at all for Thinkware, etc.). Pairs each segment's
 * master channel path with the `cameraKind` the scanner identified.
 */
export interface GpsRequest {
  path: string;
  cameraKind: CameraKind;
}

export function extractGps(
  path: string,
  cameraKind: CameraKind,
): Promise<GpsPoint[]> {
  return invoke<GpsPoint[]>("extract_gps", { path, cameraKind });
}

export function extractGpsBatch(
  requests: GpsRequest[],
): Promise<GpsBatchItem[]> {
  return invoke<GpsBatchItem[]>("extract_gps_batch", { requests });
}

/**
 * Fetch trip-stitched GPS archived in the DB (migration 0012). Returns
 * an empty array when no row exists — caller falls back to
 * `extractGpsBatch` over the segment files. Populated as a side effect
 * of timelapse encoding so the map and speed graph keep working after
 * "Delete originals".
 */
export function loadTripGps(tripId: string): Promise<GpsPoint[]> {
  return invoke<GpsPoint[]>("load_trip_gps", { tripId });
}

export interface TripGpsChannelRequest {
  label: string;
  path: string;
}

export interface TripGpsSegmentRequest {
  segmentId: string;
  durationS: number;
  cameraKind: CameraKind;
  channels: TripGpsChannelRequest[];
}

export type TripGpsProgress =
  | { kind: "started"; attempt: number; total: number }
  | { kind: "segment"; attempt: number; completed: number; total: number;
      updates: { index: number; points: GpsPoint[] }[] };

// Serialize refreshes for the same trip. Several cameras can fail in the same
// turn; an older response must not overwrite a later, more complete exclusion.
const tripGpsPending = new Map<string, Promise<GpsPoint[]>>();

/**
 * Load the persisted trip GPS when it is current. Otherwise extract the trip
 * once from the available camera files, persist it, and return the result.
 *
 * excludedLabelsBySegment is used after a runtime media failure: a camera
 * known to be corrupt for that segment is omitted, allowing another healthy
 * camera file to become the GPS source. force refreshes the DB row immediately
 * so the repaired choice is reused on the next app launch.
 */
export function ensureTripGps(
  trip: Trip,
  excludedLabelsBySegment: Record<string, string[]> = {},
  force = false,
  onProgress?: (progress: TripGpsProgress) => void,
): Promise<GpsPoint[]> {
  const segments: TripGpsSegmentRequest[] = trip.segments
    .map((segment) => {
      const excluded = new Set(excludedLabelsBySegment[segment.id] ?? []);
      return {
        segmentId: segment.id,
        durationS: segment.durationS,
        cameraKind: segment.cameraKind,
        channels: segment.channels
          .filter((channel) => !segment.isTombstone && segment.gpsSupported && !excluded.has(channel.label))
          .map((channel) => ({
            label: channel.label,
            path: channel.filePath,
          })),
      };
    });

  const previous = tripGpsPending.get(trip.id) ?? Promise.resolve([]);
  const pending = previous.catch(() => []).then(() => {
    const args: Record<string, unknown> = { tripId: trip.id, segments, force };
    if (onProgress) {
      const channel = new Channel<TripGpsProgress>();
      channel.onmessage = onProgress;
      args.onProgress = channel;
    }
    return invoke<GpsPoint[]>("ensure_trip_gps", args);
  });
  tripGpsPending.set(trip.id, pending);
  void pending.finally(() => {
    if (tripGpsPending.get(trip.id) === pending) tripGpsPending.delete(trip.id);
  }).catch(() => {});
  return pending;
}

/**
 * Write a diagnostic dump of a Miltona `.MOV` file's `gps0` atom. Used by
 * the "Export GPS debug" UI button to collect ground-truth samples while
 * the lat/lon encoding is still being finalized. Returns the path of the
 * written text file so the UI can show it / offer to open it.
 */
export function dumpMiltonaGpsDebug(path: string): Promise<string> {
  return invoke<string>("dump_miltona_gps_debug", { path });
}
