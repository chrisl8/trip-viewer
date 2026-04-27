import clsx from "clsx";
import type { PlaybackSlice } from "../../state/store";

type SourceMode = PlaybackSlice["sourceMode"];

export interface SourceOption {
  /** The mode the button represents. */
  mode: SourceMode;
  /** Enabled when the source is actually playable. Original is always
   *  enabled whenever a trip is loaded; tiers require all three
   *  channels (or at least one) done for this trip. */
  enabled: boolean;
  /** Hover tooltip for why a tier is disabled. Undefined when enabled. */
  disabledReason?: string;
}

interface Props {
  /** Active source mode from the store. */
  current: SourceMode;
  /** Four entries: Original, 8x, 16x, 60x, in that order. Disabled
   *  tiers are rendered dimmed with a tooltip. */
  options: SourceOption[];
  /** Fires when the user clicks a different source. PlayerShell owns
   *  the trip-time preserve + seek dance. */
  onChange: (mode: SourceMode) => void;
  /** Whether the controls should be grayed out entirely (no trip loaded,
   *  or engine not ready). */
  disabled?: boolean;
}

// Descriptive labels match the TimelapseView tier copy so the picker
// reads the same in both places — "8× Daily" here is the same thing
// you ticked "8x — daily review" for in the library view.
const LABEL: Record<SourceMode, string> = {
  original: "Original",
  "8x": "8× Daily",
  "16x": "16× Quick",
  "60x": "60× Year",
};

export function SourceControls({ current, options, onChange, disabled }: Props) {
  return (
    <div className="flex shrink-0 items-center gap-1">
      <span className="mr-1 text-[10px] uppercase tracking-wide text-neutral-500">
        Mode
      </span>
      {options.map((opt) => {
        const active = opt.mode === current;
        const clickable = opt.enabled && !disabled;
        return (
          <button
            key={opt.mode}
            onClick={() => clickable && onChange(opt.mode)}
            disabled={!clickable}
            title={!opt.enabled ? opt.disabledReason : undefined}
            className={clsx(
              "rounded px-2 py-1 text-xs font-medium transition-colors",
              active && clickable && "bg-violet-600 text-white",
              !active && clickable && "bg-neutral-800 text-neutral-300 hover:bg-neutral-700",
              !clickable && "cursor-not-allowed bg-neutral-900 text-neutral-600",
            )}
          >
            {LABEL[opt.mode]}
          </button>
        );
      })}
    </div>
  );
}
