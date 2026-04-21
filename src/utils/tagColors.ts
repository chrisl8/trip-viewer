import type { TagCategory } from "../types/model";

/**
 * Built-in tag name -> category mapping. Keep in sync with
 * `src-tauri/src/tags/vocabulary.rs::BUILTIN_TAGS`. User-defined tags
 * that aren't in this map fall back to "user" category.
 */
export const BUILTIN_TAG_CATEGORY: Record<string, TagCategory> = {
  event: "event",
  stationary: "motion",
  silent: "audio",
  no_audio: "audio",
  keep: "user",
};

export function categoryForTag(name: string): TagCategory {
  if (name.startsWith("place_")) return "place";
  return BUILTIN_TAG_CATEGORY[name] ?? "user";
}

/**
 * Render a tag name for the UI. Built-in tags keep their
 * underscored-lowercase form turned into spaces ("no_audio" →
 * "no audio"). Place tags (`place_<id>`) are resolved to the user's
 * display name via the places map; unknown place IDs fall back to
 * "unknown place" to avoid leaking raw IDs.
 */
export function displayNameForTag(
  name: string,
  placesById: Record<number, { name: string }>,
): string {
  if (name.startsWith("place_")) {
    const id = Number(name.slice("place_".length));
    if (Number.isFinite(id) && placesById[id]) {
      return placesById[id].name;
    }
    return "unknown place";
  }
  return name.replace(/_/g, " ");
}

/**
 * Category-to-color mapping. `band` is the Tailwind class for the
 * timeline color strip; `bg`/`text` are used by the TagBadge pill and
 * sidebar badges.
 *
 * Keep in sync with `src-tauri/src/tags/vocabulary.rs` — when a new
 * built-in tag is added, make sure its category has an entry here.
 */
export const CATEGORY_COLORS: Record<
  TagCategory,
  {
    band: string;
    bg: string;
    text: string;
    border: string;
    hoverBorder: string;
    hoverText: string;
    hex: string;
  }
> = {
  event: {
    band: "bg-amber-500",
    bg: "bg-amber-950",
    text: "text-amber-300",
    border: "border-amber-500",
    hoverBorder: "hover:border-amber-500",
    hoverText: "hover:text-amber-300",
    hex: "#f59e0b",
  },
  motion: {
    band: "bg-sky-500",
    bg: "bg-sky-950",
    text: "text-sky-300",
    border: "border-sky-500",
    hoverBorder: "hover:border-sky-500",
    hoverText: "hover:text-sky-300",
    hex: "#0ea5e9",
  },
  audio: {
    band: "bg-violet-500",
    bg: "bg-violet-950",
    text: "text-violet-300",
    border: "border-violet-500",
    hoverBorder: "hover:border-violet-500",
    hoverText: "hover:text-violet-300",
    hex: "#8b5cf6",
  },
  quality: {
    band: "bg-orange-500",
    bg: "bg-orange-950",
    text: "text-orange-300",
    border: "border-orange-500",
    hoverBorder: "hover:border-orange-500",
    hoverText: "hover:text-orange-300",
    hex: "#f97316",
  },
  user: {
    band: "bg-emerald-500",
    bg: "bg-emerald-950",
    text: "text-emerald-300",
    border: "border-emerald-500",
    hoverBorder: "hover:border-emerald-500",
    hoverText: "hover:text-emerald-300",
    hex: "#10b981",
  },
  place: {
    band: "bg-rose-500",
    bg: "bg-rose-950",
    text: "text-rose-300",
    border: "border-rose-500",
    hoverBorder: "hover:border-rose-500",
    hoverText: "hover:text-rose-300",
    hex: "#f43f5e",
  },
};
