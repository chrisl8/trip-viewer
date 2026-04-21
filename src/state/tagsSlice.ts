import type { Place } from "../ipc/places";
import type { UserApplicableTag } from "../ipc/tags";
import type { Tag } from "../types/model";

/**
 * Runtime tag state keyed by segment ID and trip ID. Kept separate from
 * the Trip/Segment objects so scan completion and user-tag edits don't
 * require rebuilding the trip tree on every change.
 */
/**
 * `{ tripId: { tagName: count } }`. Used for sidebar badges. Loaded
 * lazily after folder scans and refreshed after analysis scans complete.
 */
export type TripTagCounts = Record<string, Record<string, number>>;

export interface TagsSlice {
  tagsBySegmentId: Record<string, Tag[]>;
  tagsByTripId: Record<string, Tag[]>;
  tagsLoadingTripId: string | null;
  tripTagCounts: TripTagCounts;
  /** Developer-curated list of tags the user can apply (e.g. `parked`,
   *  `keep`). Loaded once at app startup from the Rust vocabulary. */
  userApplicableTags: UserApplicableTag[];
  /** Saved places keyed by ID for fast `place_<id>` → display-name
   *  resolution. Kept as both a list and a map — the list drives the
   *  Places view's table rendering; the map serves hot-path lookups. */
  places: Place[];
  placesById: Record<number, Place>;

  refreshTripTags: (tripId: string) => Promise<void>;
  refreshTripTagCounts: () => Promise<void>;
  loadUserApplicableTags: () => Promise<void>;
  refreshPlaces: () => Promise<void>;
  clearTags: () => void;
}
