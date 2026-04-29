import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import {
  listScans,
  type ScanCoverage,
  type ScanDescriptor,
  type ScanScope,
} from "../../ipc/scanner";
import { useStore } from "../../state/store";
import { CATEGORY_COLORS, categoryForTag } from "../../utils/tagColors";
import { formatTripStart } from "../../utils/format";
import { TripActionsMenu } from "../trip/TripActionsMenu";

type PillState = "done" | "stale" | "partial" | "failed" | "notRun";

// Reduce the four-bucket tally to a single dominant state. Failures
// take priority because they're the most actionable; an entirely
// untouched (trip, scan) is "notRun" rather than "partial" so the
// user can tell "I haven't run this yet" from "this is mid-progress."
function pillState(c: ScanCoverage): PillState {
  if (c.failedCount > 0) return "failed";
  if (c.totalSegments === 0) return "notRun";
  if (c.notRunCount === c.totalSegments) return "notRun";
  if (c.notRunCount > 0) return "partial";
  if (c.staleCount > 0) return "stale";
  return "done";
}

const PILL_CLASSES: Record<PillState, string> = {
  done: "border-emerald-700 bg-emerald-950/60 text-emerald-200",
  stale: "border-yellow-700 bg-yellow-950/60 text-yellow-200",
  partial: "border-orange-700 bg-orange-950/60 text-orange-200",
  failed: "border-red-700 bg-red-950/60 text-red-200",
  notRun: "border-neutral-700 bg-neutral-900 text-neutral-500",
};

const PILL_GLYPH: Record<PillState, string> = {
  done: "✓",
  stale: "⚠",
  partial: "◐",
  failed: "✗",
  notRun: "○",
};

function pillTooltip(displayName: string, c: ScanCoverage): string {
  const state = pillState(c);
  const stateLabel: Record<PillState, string> = {
    done: "complete",
    stale: "stale (algorithm version bumped)",
    partial: "partial",
    failed: "failed",
    notRun: "not run",
  };
  const parts = [`${displayName} — ${stateLabel[state]}`];
  if (c.totalSegments > 0) {
    const tally = `${c.doneCount}/${c.totalSegments} segments done`;
    const extras: string[] = [];
    if (c.staleCount > 0) extras.push(`${c.staleCount} stale`);
    if (c.failedCount > 0) extras.push(`${c.failedCount} failed`);
    if (c.notRunCount > 0 && c.notRunCount < c.totalSegments) {
      extras.push(`${c.notRunCount} not run`);
    }
    parts.push(extras.length > 0 ? `${tally} · ${extras.join(" · ")}` : tally);
  }
  if (c.sampleFailures.length > 0) {
    parts.push("");
    parts.push("Errors:");
    for (const msg of c.sampleFailures) parts.push(`• ${msg}`);
  }
  return parts.join("\n");
}

const SCOPE_LABELS: { value: ScanScope; label: string; hint: string }[] = [
  {
    value: "newOnly",
    label: "New segments only",
    hint: "Scan only segments that haven't been processed by the selected scans.",
  },
  {
    value: "rescanStale",
    label: "Rescan stale",
    hint: "Also re-run scans whose algorithm version has been bumped since last run.",
  },
  {
    value: "all",
    label: "Scan all",
    hint: "Ignore previous scan state and scan everything. Slowest option.",
  },
];

function formatDurationShort(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  const totalSec = Math.round(ms / 1000);
  if (totalSec < 60) return `${totalSec}s`;
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  if (mins < 60) return secs === 0 ? `${mins}m` : `${mins}m ${secs}s`;
  const hrs = Math.floor(mins / 60);
  const remMins = mins % 60;
  return remMins === 0 ? `${hrs}h` : `${hrs}h ${remMins}m`;
}

export function ScanView() {
  const setMainView = useStore((s) => s.setMainView);
  const running = useStore((s) => s.scanRunning);
  const progress = useStore((s) => s.scanProgress);
  const lastResult = useStore((s) => s.scanLastResult);
  const startMs = useStore((s) => s.scanStartMs);
  const startScan = useStore((s) => s.startAnalysisScan);
  const cancelScan = useStore((s) => s.cancelAnalysisScan);
  const trips = useStore((s) => s.trips);
  const coverage = useStore((s) => s.scanCoverage);
  const refreshCoverage = useStore((s) => s.refreshScanCoverage);
  const places = useStore((s) => s.places);

  // Tick once per second while a scan is running so the ETA display
  // ticks down in real time even when no new progress event has landed.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [running]);

  const [scans, setScans] = useState<ScanDescriptor[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [scope, setScope] = useState<ScanScope>("newOnly");
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    listScans()
      .then((descriptors) => {
        setScans(descriptors);
        // Default: pre-select every scan so the user can just click Start.
        setSelected(new Set(descriptors.map((d) => d.id)));
      })
      .catch((e) => {
        setLoadError(String(e));
      });
    void refreshCoverage();
  }, [refreshCoverage]);

  // Poll the coverage matrix while a scan is running so the Trips
  // table reflects rows flipping to done/stale/failed live. The query
  // is two GROUP BY scans plus a HashMap merge — cheap. The cleanup
  // also refreshes once to catch the final transitions.
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => void refreshCoverage(), 1500);
    return () => {
      clearInterval(interval);
      void refreshCoverage();
    };
  }, [running, refreshCoverage]);

  const coverageByTrip = useMemo(() => {
    const m: Record<string, ScanCoverage[]> = {};
    for (const t of coverage) m[t.tripId] = t.perScan;
    return m;
  }, [coverage]);

  const scansById = useMemo(() => {
    const m: Record<string, ScanDescriptor> = {};
    for (const s of scans) m[s.id] = s;
    return m;
  }, [scans]);

  // Map segmentId → tripId so the live progress event's
  // currentSegmentId can be resolved to the trip whose row should
  // light up. Trips and their segments are already in memory; this
  // is just a flat index over them.
  const segmentToTripId = useMemo(() => {
    const m = new Map<string, string>();
    for (const t of trips) {
      for (const s of t.segments) m.set(s.id, t.id);
    }
    return m;
  }, [trips]);

  const runningTripId =
    running && progress?.currentSegmentId
      ? (segmentToTripId.get(progress.currentSegmentId) ?? null)
      : null;
  const runningScanId = (running && progress?.currentScanId) || null;

  const canStart = selected.size > 0 && !running && scans.length > 0;
  const doneCount = progress?.done ?? 0;
  const total = progress?.total ?? 0;
  const pct = total > 0 ? Math.round((doneCount / total) * 100) : 0;

  // ETA: running-average. Unreliable before ~5 items have completed —
  // the first Cheap scans race through while Heavy ones haven't started
  // yet — so show "calculating…" until we have a stable sample.
  const etaLabel = useMemo(() => {
    if (!running || !startMs || total === 0) return null;
    if (doneCount < 5) return "calculating…";
    const elapsed = now - startMs;
    const avgPer = elapsed / doneCount;
    const remaining = total - doneCount;
    if (remaining <= 0) return null;
    return formatDurationShort(avgPer * remaining);
  }, [running, startMs, total, doneCount, now]);

  async function onStart() {
    try {
      await startScan(Array.from(selected), scope);
    } catch (e) {
      setLoadError(String(e));
    }
  }

  async function onRebuildTrip(tripId: string) {
    try {
      // newOnly would silently no-op for an already-scanned trip,
      // which is the opposite of what "Rebuild" means. Coerce to
      // "all"; pass rescanStale/all through unchanged. Same fix as
      // the Timelapse per-trip rebuild button.
      const effectiveScope: ScanScope = scope === "newOnly" ? "all" : scope;
      await startScan(Array.from(selected), effectiveScope, [tripId]);
    } catch (e) {
      setLoadError(String(e));
    }
  }

  const rebuildDisabledReason = running
    ? "Scan in progress"
    : selected.size === 0
      ? "Pick at least one scan"
      : null;

  return (
    <div className="flex h-full flex-col overflow-hidden bg-neutral-950 text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
        <div>
          <h1 className="text-lg font-semibold">Scan library</h1>
          <p className="text-xs text-neutral-500">
            Analyze segments to attach tags. Tags surface in the sidebar,
            timeline, and Review view.
          </p>
        </div>
        <button
          onClick={() => setMainView("player")}
          className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
        >
          Close
        </button>
      </header>

      <div className="flex-1 overflow-y-auto p-4">
        {loadError && (
          <div className="mb-4 rounded-md bg-red-950 px-3 py-2 text-sm text-red-300">
            {loadError}
          </div>
        )}

        <section className="mb-6">
          <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
            Scans to run
          </h2>
          {scans.length === 0 && !loadError && (
            <p className="text-sm text-neutral-500">Loading…</p>
          )}
          <ul className="flex flex-col gap-2">
            {scans.map((scan) => {
              const checked = selected.has(scan.id);
              return (
                <li key={scan.id}>
                  <label className="flex cursor-pointer items-start gap-3 rounded-md border border-neutral-800 bg-neutral-900 p-3 hover:border-neutral-700">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => {
                        const next = new Set(selected);
                        if (checked) next.delete(scan.id);
                        else next.add(scan.id);
                        setSelected(next);
                      }}
                      disabled={running}
                      className="mt-0.5"
                    />
                    <div className="flex-1">
                      <div className="flex items-baseline gap-2">
                        <span className="font-medium">{scan.displayName}</span>
                        <span className="text-xs text-neutral-500">
                          {scan.costTier}
                        </span>
                      </div>
                      <p className="mt-0.5 text-xs text-neutral-400">
                        {scan.description}
                      </p>
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
                        <span className="text-[10px] uppercase tracking-wide text-neutral-600">
                          Emits
                        </span>
                        {scan.emits.map((name) => {
                          const colors = CATEGORY_COLORS[categoryForTag(name)];
                          return (
                            <span
                              key={name}
                              className={clsx(
                                "rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide",
                                colors.bg,
                                colors.text,
                              )}
                            >
                              {name.replace(/_/g, " ")}
                            </span>
                          );
                        })}
                      </div>
                      {scan.id === "gps_place" && places.length === 0 && (
                        <p className="mt-2 rounded-md bg-amber-950/60 px-2 py-1 text-[11px] text-amber-300">
                          No places defined yet — this scan won&apos;t emit
                          any tags until you add at least one. Open{" "}
                          <span className="font-medium">Places</span> in the
                          sidebar footer to define points of interest.
                        </p>
                      )}
                    </div>
                  </label>
                </li>
              );
            })}
          </ul>
        </section>

        <section className="mb-6">
          <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
            Scope
          </h2>
          <div className="flex flex-col gap-2">
            {SCOPE_LABELS.map((opt) => (
              <label
                key={opt.value}
                className="flex cursor-pointer items-start gap-3 rounded-md border border-neutral-800 bg-neutral-900 p-3 hover:border-neutral-700"
              >
                <input
                  type="radio"
                  name="scope"
                  checked={scope === opt.value}
                  onChange={() => setScope(opt.value)}
                  disabled={running}
                  className="mt-0.5"
                />
                <div>
                  <div className="font-medium">{opt.label}</div>
                  <div className="text-xs text-neutral-500">{opt.hint}</div>
                </div>
              </label>
            ))}
          </div>
        </section>

        {(running || progress) && (
          <section className="mb-6">
            <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
              Progress
            </h2>
            <div className="rounded-md border border-neutral-800 bg-neutral-900 p-3">
              <div className="mb-2 h-2 w-full overflow-hidden rounded-full bg-neutral-800">
                <div
                  className="h-full bg-sky-500 transition-all"
                  style={{ width: `${pct}%` }}
                />
              </div>
              <div className="flex items-center justify-between text-xs text-neutral-400">
                <span>
                  {doneCount} / {total} ({pct}%)
                </span>
                <div className="flex items-center gap-3">
                  {etaLabel && (
                    <span>
                      ETA{" "}
                      <span className="text-neutral-200">{etaLabel}</span>
                    </span>
                  )}
                  <span>{progress?.failed ?? 0} failed</span>
                </div>
              </div>
              {progress?.currentScanId && (
                <div className="mt-1 truncate text-xs text-neutral-500">
                  {progress.currentScanId} · {progress.currentSegmentId}
                </div>
              )}
            </div>
          </section>
        )}

        {lastResult && !running && (
          <section className="mb-6 rounded-md border border-neutral-800 bg-neutral-900 p-3 text-sm">
            <div className="font-medium">
              {lastResult.cancelled ? "Scan cancelled" : "Scan complete"}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              {lastResult.done} scanned · {lastResult.tagsEmitted} tags
              emitted · {lastResult.failed} failed
            </div>
          </section>
        )}

        {trips.length > 0 && scans.length > 0 && (
          <section className="mb-6">
            <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
              Trips
            </h2>
            <div className="overflow-hidden rounded-md border border-neutral-800">
              <table className="w-full text-sm">
                <thead className="bg-neutral-900 text-xs uppercase tracking-wide text-neutral-500">
                  <tr>
                    <th className="px-3 py-2 text-left">Trip</th>
                    <th className="px-3 py-2 text-left">Segments</th>
                    <th className="px-3 py-2 text-left">Coverage</th>
                    <th className="px-3 py-2 text-left">Rebuild</th>
                    <th className="px-3 py-2 text-left"></th>
                  </tr>
                </thead>
                <tbody>
                  {trips.map((t) => {
                    const perScan = coverageByTrip[t.id] ?? [];
                    const isTripRunning = t.id === runningTripId;
                    return (
                      <tr
                        key={t.id}
                        className={clsx(
                          "border-t border-neutral-800 hover:bg-neutral-900/60",
                          // Subtle row tint when this trip is currently
                          // being scanned — overrides the default hover
                          // background so the highlight stays visible
                          // even on hover.
                          isTripRunning && "bg-sky-950/30",
                        )}
                      >
                        <td className="px-3 py-2">
                          <div className="truncate text-neutral-200">
                            {formatTripStart(t.startTime)}
                          </div>
                          <div className="text-xs text-neutral-500">
                            {t.id.slice(0, 8)}…
                          </div>
                        </td>
                        <td className="px-3 py-2 text-neutral-400">
                          {t.segments.length}
                        </td>
                        <td className="px-3 py-2">
                          {perScan.length === 0 ? (
                            <span className="text-xs text-neutral-500">—</span>
                          ) : (
                            <div className="flex flex-wrap gap-1">
                              {perScan.map((c) => {
                                const scan = scansById[c.scanId];
                                const label = scan?.displayName ?? c.scanId;
                                const state = pillState(c);
                                // Live overlay: the (trip, scan) currently
                                // being processed gets a sky-blue pulsing
                                // pill regardless of the underlying coverage
                                // state. scan_runs only records final state,
                                // so "running" is overlay info from the
                                // progress event, not a coverage bucket.
                                const isPillRunning =
                                  isTripRunning && c.scanId === runningScanId;
                                return (
                                  <span
                                    key={c.scanId}
                                    title={
                                      isPillRunning
                                        ? `${label} — running now`
                                        : pillTooltip(label, c)
                                    }
                                    className={clsx(
                                      "inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium",
                                      isPillRunning
                                        ? "animate-pulse-sky border-sky-500 bg-sky-950/60 text-sky-200"
                                        : PILL_CLASSES[state],
                                    )}
                                  >
                                    <span aria-hidden="true">
                                      {isPillRunning ? "▶" : PILL_GLYPH[state]}
                                    </span>
                                    {label}
                                  </span>
                                );
                              })}
                            </div>
                          )}
                        </td>
                        <td className="px-3 py-2">
                          <button
                            onClick={() => void onRebuildTrip(t.id)}
                            disabled={rebuildDisabledReason !== null}
                            className={clsx(
                              "rounded px-2 py-1 text-xs font-medium transition-colors",
                              rebuildDisabledReason === null
                                ? "bg-neutral-700 text-neutral-100 hover:bg-neutral-600"
                                : "cursor-not-allowed bg-neutral-800 text-neutral-600",
                            )}
                            title={
                              rebuildDisabledReason ??
                              (scope === "newOnly"
                                ? "Re-run the selected scans on this trip (forces re-scan — scope is set to New segments only, which would otherwise skip done rows)"
                                : `Re-run the selected scans on this trip (scope: ${scope === "rescanStale" ? "Rescan stale" : "Scan all"})`)
                            }
                          >
                            ↻
                          </button>
                        </td>
                        <td className="px-3 py-2">
                          <TripActionsMenu trip={t} variant="icon" />
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </section>
        )}
      </div>

      <footer className="flex items-center justify-end gap-2 border-t border-neutral-800 px-4 py-3">
        {running ? (
          <button
            onClick={() => void cancelScan()}
            className="rounded-md bg-red-700 px-4 py-2 text-sm font-medium text-white hover:bg-red-600"
          >
            Cancel
          </button>
        ) : (
          <button
            onClick={() => void onStart()}
            disabled={!canStart}
            className={clsx(
              "rounded-md px-4 py-2 text-sm font-medium",
              canStart
                ? "bg-sky-600 text-white hover:bg-sky-500"
                : "cursor-not-allowed bg-neutral-800 text-neutral-500",
            )}
          >
            Start scan
          </button>
        )}
      </footer>
    </div>
  );
}
