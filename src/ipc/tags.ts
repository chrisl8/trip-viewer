import { invoke } from "@tauri-apps/api/core";
import type { Tag, TagCategory } from "../types/model";
import type { TripTagCounts } from "../state/tagsSlice";

export interface UserApplicableTag {
  name: string;
  category: TagCategory;
  displayName: string;
  description: string;
}

export function listUserApplicableTags(): Promise<UserApplicableTag[]> {
  return invoke<UserApplicableTag[]>("list_user_applicable_tags");
}

export function getTagsForTrip(tripId: string): Promise<Tag[]> {
  return invoke<Tag[]>("get_tags_for_trip", { tripId });
}

export function getTagCountsByTrip(): Promise<TripTagCounts> {
  return invoke<TripTagCounts>("get_tag_counts_by_trip");
}

export function getAllTags(): Promise<Tag[]> {
  return invoke<Tag[]>("get_all_tags");
}

export function addUserTag(
  segmentIds: string[],
  name: string,
  note?: string,
): Promise<number> {
  return invoke<number>("add_user_tag", { segmentIds, name, note });
}

export function removeUserTag(
  segmentIds: string[],
  name: string,
): Promise<number> {
  return invoke<number>("remove_user_tag", { segmentIds, name });
}

export interface DeleteFailure {
  path: string;
  message: string;
}

export interface DeleteReport {
  segmentsRemoved: number;
  filesTrashed: number;
  failures: DeleteFailure[];
  /**
   * Segment IDs that were converted to tombstones (timeline gap kept
   * in place because the trip has a completed timelapse). The rest of
   * `segmentsRemoved` were hard-deleted; the frontend splices those
   * out and converts these to `isTombstone: true` in place.
   */
  tombstonedSegmentIds: string[];
}

export function deleteSegmentsToTrash(
  segmentIds: string[],
  inMemoryPaths: Record<string, string[]>,
): Promise<DeleteReport> {
  return invoke<DeleteReport>("delete_segments_to_trash", {
    segmentIds,
    inMemoryPaths,
  });
}
