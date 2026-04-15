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
      {drift.length === 0 ? (
        <div className="text-neutral-500">no slaves</div>
      ) : (
        drift.map((d) => {
          const warn = d.driftMs > 40 || d.driftMs < -40;
          return (
            <div
              key={d.label}
              className={warn ? "text-yellow-400" : "text-green-400"}
            >
              {d.label}: {d.driftMs > 0 ? "+" : ""}
              {d.driftMs}ms
            </div>
          );
        })
      )}
    </div>
  );
}
