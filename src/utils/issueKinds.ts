import type { ScanError, ScanErrorKind } from "../types/model";

/**
 * Per-kind display metadata: short pill label + Tailwind pill classes.
 * Colors are graded by severity/recoverability:
 *   amber  = probably recoverable with external tools (missing index)
 *   red    = structurally broken
 *   neutral= we understand the file but it isn't usable here
 */
export const KIND_META: Record<ScanErrorKind, { label: string; className: string }> = {
  invalidFilename: {
    label: "Bad name",
    className: "bg-neutral-800 text-neutral-300",
  },
  fileUnreadable: {
    label: "Unreadable",
    className: "bg-red-950 text-red-300",
  },
  mp4MoovMissing: {
    label: "No index",
    className: "bg-amber-950 text-amber-300",
  },
  mp4BoxOverflow: {
    label: "Corrupted",
    className: "bg-red-950 text-red-300",
  },
  mp4NoVideoTrack: {
    label: "No video",
    className: "bg-neutral-800 text-neutral-300",
  },
  mp4Other: {
    label: "MP4 error",
    className: "bg-red-950 text-red-300",
  },
};

/**
 * Count errors per kind, sorted by count descending. Returns only kinds
 * that have at least one matching error.
 */
export function kindCounts(
  errors: ScanError[],
): Array<{ kind: ScanErrorKind; count: number }> {
  const map = new Map<ScanErrorKind, number>();
  for (const e of errors) {
    map.set(e.kind, (map.get(e.kind) ?? 0) + 1);
  }
  const out = Array.from(map.entries()).map(([kind, count]) => ({ kind, count }));
  out.sort((a, b) => b.count - a.count);
  return out;
}
