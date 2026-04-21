import clsx from "clsx";
import { useStore } from "../../state/store";
import {
  CATEGORY_COLORS,
  categoryForTag,
  displayNameForTag,
} from "../../utils/tagColors";

interface Props {
  tripId: string;
  /** Maximum number of tag counts to show before collapsing into "+N more". */
  max?: number;
}

/**
 * Inline per-trip tag breakdown rendered under each trip row in the
 * sidebar. Colors the name by category so users can spot "mostly junk"
 * vs. "mostly event" trips at a glance.
 */
export function TripBadges({ tripId, max = 3 }: Props) {
  const counts = useStore((s) => s.tripTagCounts[tripId]);
  const placesById = useStore((s) => s.placesById);
  if (!counts) return null;

  const entries = Object.entries(counts).sort((a, b) => b[1] - a[1]);
  if (entries.length === 0) return null;
  const shown = entries.slice(0, max);
  const extra = entries.length - shown.length;

  return (
    <div className="flex flex-wrap gap-x-2 text-[11px]">
      {shown.map(([name, count]) => {
        const category = categoryForTag(name);
        const colors = CATEGORY_COLORS[category];
        return (
          <span key={name} className={clsx(colors.text)}>
            {count} {displayNameForTag(name, placesById)}
          </span>
        );
      })}
      {extra > 0 && (
        <span className="text-neutral-500">+{extra} more</span>
      )}
    </div>
  );
}
