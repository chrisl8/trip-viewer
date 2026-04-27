import clsx from "clsx";
import type { SyncEngine } from "../../engine/SyncEngine";
import { useStore } from "../../state/store";

const SPEEDS: Array<0.5 | 1 | 2 | 4> = [0.5, 1, 2, 4];

interface Props {
  engine: SyncEngine | null;
}

export function SpeedControls({ engine }: Props) {
  const speed = useStore((s) => s.speed);
  const setSpeed = useStore((s) => s.setSpeed);

  return (
    <div className="flex shrink-0 items-center gap-1">
      <span className="mr-1 text-[10px] uppercase tracking-wide text-neutral-500">
        Speed
      </span>
      {SPEEDS.map((s) => (
        <button
          key={s}
          onClick={() => {
            setSpeed(s);
            engine?.setSpeed(s);
          }}
          className={clsx(
            "rounded px-2 py-1 text-xs font-medium transition-colors",
            speed === s
              ? "bg-blue-600 text-white"
              : "bg-neutral-800 text-neutral-300 hover:bg-neutral-700",
          )}
        >
          ×{s}
        </button>
      ))}
    </div>
  );
}
