import { useStore } from "../../state/store";

export function DriftHud() {
  const show = useStore((s) => s.showDriftHud);
  const drift = useStore((s) => s.drift);

  if (!show) return null;

  return (
    <div className="pointer-events-none absolute left-3 top-3 z-[1000] rounded-md bg-black/80 px-3 py-2 font-mono text-xs backdrop-blur">
      <div className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-neutral-500">
        Sync Drift
      </div>
      <div className={drift.interior > 40 || drift.interior < -40 ? "text-yellow-400" : "text-green-400"}>
        Interior: {drift.interior > 0 ? "+" : ""}{drift.interior}ms
      </div>
      <div className={drift.rear > 40 || drift.rear < -40 ? "text-yellow-400" : "text-green-400"}>
        Rear: {drift.rear > 0 ? "+" : ""}{drift.rear}ms
      </div>
    </div>
  );
}
