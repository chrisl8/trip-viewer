import clsx from "clsx";
import { useStore } from "../../state/store";
import type { Tag } from "../../types/model";
import { CATEGORY_COLORS, displayNameForTag } from "../../utils/tagColors";

interface Props {
  tag: Tag;
  compact?: boolean;
  onClick?: () => void;
}

export function TagBadge({ tag, compact = false, onClick }: Props) {
  const placesById = useStore((s) => s.placesById);
  const colors = CATEGORY_COLORS[tag.category];
  const label = displayNameForTag(tag.name, placesById);
  const title = [
    `${tag.name} (${tag.category})`,
    `source: ${tag.source}`,
    tag.scanId ? `scan: ${tag.scanId} v${tag.scanVersion ?? "?"}` : null,
    tag.confidence != null ? `confidence: ${tag.confidence.toFixed(2)}` : null,
    tag.note,
  ]
    .filter(Boolean)
    .join("\n");

  const Component = onClick ? "button" : "span";
  return (
    <Component
      onClick={onClick}
      title={title}
      className={clsx(
        "inline-flex items-center rounded-full font-medium uppercase tracking-wide",
        compact ? "px-1.5 py-0.5 text-[10px]" : "px-2 py-0.5 text-xs",
        colors.bg,
        colors.text,
        onClick && "cursor-pointer hover:brightness-125",
      )}
    >
      {label}
    </Component>
  );
}
