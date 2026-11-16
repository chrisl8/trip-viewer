import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import type {
  TimelapseChannel,
  TimelapseJobScope,
  TimelapseTier,
} from "../../ipc/timelapse";
import { pruneOrphanTimelapseFiles } from "../../ipc/timelapse";
import { formatBytes } from "../../utils/format";
import { formatTripStart } from "../../utils/format";
import { parseCurveJson } from "../../utils/speedCurve";
import { fallbackCurveForTier } from "../../utils/tripTime";
import { estimateEncodingStep } from "./encodingProgress";
import { encoderLabel } from "./encoderLabel";
import { TripActionsMenu } from "../trip/TripActionsMenu";
import { FfmpegConfig } from "./FfmpegConfig";

// State used by both the per-trip status pills and the library-wide
// "Overall coverage" summary. Maps onto the same colors and glyphs the
// Scan view uses so the visual vocabulary stays consistent across tabs.
type CoverageState = "done" | "running" | "partial" | "failed" | "notRun";

const COVERAGE_PILL_CLASSES: Record<CoverageState, string> = {
  done: "border-emerald-700 bg-emerald-950/60 text-emerald-200",
  running: "border-sky-700 bg-sky-950/60 text-sky-200",
  partial: "border-orange-700 bg-orange-950/60 text-orange-200",
  failed: "border-red-700 bg-red-950/60 text-red-200",
  notRun: "border-neutral-700 bg-neutral-900 text-neutral-500",
};

const COVERAGE_PILL_GLYPH: Record<CoverageState, string> = {
  done: "✓",
  running: "▶",
  partial: "◐",
  failed: "✗",
  notRun: "○",
};

const COVERAGE_STATE_LABEL: Record<CoverageState, string> = {
  done: "playable",
  running: "running",
  partial: "partial",
  failed: "failed",
  notRun: "not run",
};

const TIER_OPTIONS: {
  value: TimelapseTier;
  label: string;
  hint: string;
}[] = [
  {
    value: "8x",
    label: "8x — daily review",
    hint: "Fixed 8x throughout, no slowdowns. Steady fast-forward review — skim a half-hour trip in about four minutes.",
  },
  {
    value: "16x",
    label: "16x — quick scan",
    hint: "Base 16x, drops to 1x during GPS-detected events (hard brake, sharp turn, long stop, traffic).",
  },
  {
    value: "60x",
    label: "60x — year in review",
    hint: "Base 60x, drops to 8x during the same GPS-detected events. Cinematic pacing for month- and year-scale browsing.",
  },
];

const CHANNEL_OPTIONS: {
  value: TimelapseChannel;
  label: string;
}[] = [
  { value: "F", label: "Front" },
  { value: "I", label: "Interior" },
  { value: "R", label: "Rear" },
];

const SCOPE_OPTIONS: {
  value: TimelapseJobScope;
  label: string;
  hint: string;
}[] = [
  {
    value: "newOnly",
    label: "New & unfinished",
    hint: "Encode anything that hasn't completed yet. Skips done rows; picks up fresh, pending, and cancelled work. The usual choice — including after a cancel.",
  },
  {
    value: "failedOnly",
    label: "Retry failed",
    hint: "Re-run only the combinations that previously failed.",
  },
  {
    value: "rebuildAll",
    label: "Rebuild all",
    hint: "Re-encode everything the pickers above select, even if already done.",
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

export function TimelapseView() {
  const setMainView = useStore((s) => s.setMainView);
  const trips = useStore((s) => s.trips);
  const ffmpegPath = useStore((s) => s.ffmpegPath);
  const caps = useStore((s) => s.ffmpegCapabilities);
  const running = useStore((s) => s.timelapseRunning);
  const scanning = useStore((s) => s.timelapseScanning);
  const progress = useStore((s) => s.timelapseProgress);
  const lastResult = useStore((s) => s.timelapseLastResult);
  const startMs = useStore((s) => s.timelapseStartMs);
  const jobs = useStore((s) => s.timelapseJobs);
  const refreshSettings = useStore((s) => s.refreshTimelapseSettings);
  const refreshJobs = useStore((s) => s.refreshTimelapseJobs);
  const startRun = useStore((s) => s.startTimelapseRun);
  const cancelRun = useStore((s) => s.cancelTimelapseRun);
  const selectTrip = useStore((s) => s.selectTrip);
  const setSourceMode = useStore((s) => s.setSourceMode);

  const [showConfig, setShowConfig] = useState(false);
  const [pruneState, setPruneState] = useState<
    | { kind: "idle" }
    | { kind: "running" }
    | { kind: "done"; trashed: number; bytes: number; sample: string[] }
    | { kind: "error"; message: string }
  >({ kind: "idle" });
  const orphanCount = useStore((s) => s.orphanTimelapseCount);
  const [tiers, setTiers] = useState<Set<TimelapseTier>>(
    new Set(["8x", "16x", "60x"]),
  );
  const [channels, setChannels] = useState<Set<TimelapseChannel>>(
    new Set(["F", "I", "R"]),
  );
  const [scope, setScope] = useState<TimelapseJobScope>("newOnly");
  const [error, setError] = useState<string | null>(null);
  // Only decide whether to auto-open the config modal after at least
  // one settings refresh has resolved — otherwise the initial null
  // store value fires the modal open before the persisted path lands.
  const [settingsChecked, setSettingsChecked] = useState(false);

  useEffect(() => {
    void refreshSettings().finally(() => setSettingsChecked(true));
    void refreshJobs();
  }, [refreshSettings, refreshJobs]);

  // Auto-open the config modal on first visit if ffmpeg isn't set up yet.
  useEffect(() => {
    if (settingsChecked && !ffmpegPath) setShowConfig(true);
  }, [settingsChecked, ffmpegPath]);

  // Tick once per second while running so the ETA label updates even
  // between progress events.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [running]);

  // Poll the jobs table while a run is active so the Trips table
  // reflects rows flipping pending → running → done/failed live.
  // Progress events carry only summary counts, not per-row state, so
  // periodic re-query is the cheapest way to keep the table fresh.
  // The cleanup also refreshes once to catch the final transitions.
  useEffect(() => {
    if (!running) return;
    const interval = setInterval(() => void refreshJobs(), 1500);
    return () => {
      clearInterval(interval);
      void refreshJobs();
    };
  }, [running, refreshJobs]);

  const configured = ffmpegPath !== null && caps !== null;

  const doneCount = progress?.done ?? 0;
  const failedCount = progress?.failed ?? 0;
  const total = progress?.total ?? 0;
  const processedCount = doneCount + failedCount;
  const donePct = total > 0 ? (doneCount / total) * 100 : 0;
  const failedPct = total > 0 ? (failedCount / total) * 100 : 0;
  const pct = Math.round(donePct);

  const etaLabel = useMemo(() => {
    if (!running || !startMs || total === 0) return null;
    if (processedCount < 1) return null;
    const elapsed = now - startMs;
    // Amortize over attempts: elapsed includes failure time, so dividing by successes alone inflates ETA.
    const avgPer = elapsed / processedCount;
    const remaining = total - processedCount;
    if (remaining <= 0) return null;
    return formatDurationShort(avgPer * remaining);
  }, [running, startMs, total, processedCount, now]);
  const encodingProgress = useStore((s) => s.timelapseEncodingProgress);
  const encodingEstimate = encodingProgress ? estimateEncodingStep(encodingProgress, now) : null;

  const jobsByTrip = useMemo(() => {
    const m: Record<string, typeof jobs> = {};
    for (const j of jobs) {
      if (!m[j.tripId]) m[j.tripId] = [];
      m[j.tripId].push(j);
    }
    return m;
  }, [jobs]);

  // Library-wide aggregate: for each tier, how many trips are
  // playable / running / partial / failed-only / not-yet-run. "Done"
  // is defined as the per-trip Play button's definition (any channel
  // completed at this tier) — that's what makes the tier usable to
  // the user. Permanent CameraDoesNotRecord failures don't prevent
  // "done" so single-channel dashcams still read as playable once
  // Front lands.
  const overallByTier = useMemo(() => {
    const tally: Record<TimelapseTier, Record<CoverageState, number>> = {
      "8x": { done: 0, running: 0, partial: 0, failed: 0, notRun: 0 },
      "16x": { done: 0, running: 0, partial: 0, failed: 0, notRun: 0 },
      "60x": { done: 0, running: 0, partial: 0, failed: 0, notRun: 0 },
    };
    for (const trip of trips) {
      const tripJobs = jobsByTrip[trip.id] ?? [];
      for (const tier of ["8x", "16x", "60x"] as const) {
        const tjobs = tripJobs.filter((j) => j.tier === tier);
        const doneCount = tjobs.filter((j) => j.status === "done").length;
        const runningCount = tjobs.filter(
          (j) => j.status === "running",
        ).length;
        const pendingCount = tjobs.filter(
          (j) => j.status === "pending",
        ).length;
        const failedCount = tjobs.filter(
          (j) => j.status === "failed",
        ).length;
        let state: CoverageState;
        if (runningCount > 0) {
          state = "running";
        } else if (doneCount > 0 && pendingCount === 0) {
          state = "done";
        } else if (doneCount > 0) {
          state = "partial";
        } else if (failedCount > 0 && pendingCount === 0 && tjobs.length > 0) {
          state = "failed";
        } else {
          state = "notRun";
        }
        tally[tier][state] += 1;
      }
    }
    return tally;
  }, [trips, jobsByTrip]);

  const canStart =
    configured && !running && tiers.size > 0 && channels.size > 0;

  // Missing sibling channels no longer carry any penalty (no black-
  // placeholder padding, no slow CPU path — they're encoded real-footage-
  // only on the fast GPU pipeline and shown as a black overlay at
  // playback), so there's nothing to warn about pre-encode. Just start.
  async function startEncode(args: Parameters<typeof startRun>[0]) {
    setError(null);
    try {
      await startRun(args);
    } catch (e) {
      setError(String(e));
    }
  }

  async function onStart() {
    await startEncode({
      tripIds: null, // null = every trip in the library
      tiers: Array.from(tiers),
      channels: Array.from(channels),
      scope,
    });
  }

  async function onPruneOrphans() {
    if (
      !window.confirm(
        "Move orphan timelapse files to trash?\n\n" +
          "These are .mp4 files under <archive>/Timelapses/ whose trip_id " +
          "matches no row in the database — they're left over from a trip " +
          "rename or trip merge and the app no longer references them. " +
          "Files go to the OS trash so you can recover them if needed.",
      )
    ) {
      return;
    }
    setPruneState({ kind: "running" });
    try {
      const result = await pruneOrphanTimelapseFiles();
      setPruneState({
        kind: "done",
        trashed: result.trashed,
        bytes: result.bytesReclaimed,
        sample: result.sample,
      });
      // Badge tracks live disk state — refresh so it reflects what
      // the prune just did, including any orphans that couldn't be
      // trashed (errors are logged backend-side and stay counted).
      void useStore.getState().refreshOrphanCount();
    } catch (e) {
      setPruneState({ kind: "error", message: String(e) });
    }
  }

  async function onRebuildTrip(tripId: string) {
    await startEncode({
      tripIds: [tripId],
      tiers: Array.from(tiers),
      channels: Array.from(channels),
      // newOnly would no-op for an already-done trip, which is the
      // opposite of what "Rebuild" means. Coerce to rebuildAll; pass
      // failedOnly/rebuildAll through unchanged.
      scope: scope === "newOnly" ? "rebuildAll" : scope,
    });
  }

  const rebuildDisabledReason = !configured
    ? "Configure ffmpeg first"
    : running
      ? "Encoding in progress"
      : tiers.size === 0
        ? "Pick at least one tier"
        : channels.size === 0
            ? "Pick at least one channel"
            : null;

  return (
    <div className="flex h-full flex-col overflow-hidden bg-neutral-950 text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
        <div>
          <h1 className="text-lg font-semibold">Timelapse library</h1>
          <p className="text-xs text-neutral-500">
            Pre-render fast-playback versions of each trip using ffmpeg.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {configured ? (
            // Healthy state: a small unlabeled gear so the user can
            // still re-open FfmpegConfig to switch binaries, but no
            // status pill announcing the obvious "ready" state.
            <button
              onClick={() => setShowConfig(true)}
              className="rounded-md border border-neutral-700 px-2 py-1 text-sm text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
              title="Configure ffmpeg"
              aria-label="Configure ffmpeg"
            >
              ⚙
            </button>
          ) : (
            // Broken state: prominent amber-tinted button matching the
            // body banner's tone so the call-to-action is unmissable.
            <button
              onClick={() => setShowConfig(true)}
              className="rounded-md border border-amber-700/50 bg-amber-950/40 px-3 py-1 text-sm text-amber-200 hover:bg-amber-950/60"
              title="Configure ffmpeg"
            >
              ffmpeg: not set
            </button>
          )}
        </div>
      </header>

      <div className="flex-1 overflow-y-auto p-4">
        {!configured && (
          <div className="mb-4 rounded-md border border-amber-700/50 bg-amber-950/40 px-3 py-2 text-sm text-amber-200">
            Timelapse generation needs an ffmpeg binary. Click{" "}
            <span className="font-medium">ffmpeg: not set</span> above to
            point the app at one.
          </div>
        )}
        {error && (
          <div className="mb-4 rounded-md bg-red-950 px-3 py-2 text-sm text-red-300">
            {error}
          </div>
        )}

        <section className="mb-6">
          <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
            Tiers
          </h2>
          <ul className="flex flex-col gap-2">
            {TIER_OPTIONS.map((opt) => {
              const checked = tiers.has(opt.value);
              return (
                <li key={opt.value}>
                  <label className="flex cursor-pointer items-start gap-3 rounded-md border border-neutral-800 bg-neutral-900 p-3 hover:border-neutral-700">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => {
                        const next = new Set(tiers);
                        if (checked) next.delete(opt.value);
                        else next.add(opt.value);
                        setTiers(next);
                      }}
                      disabled={running}
                      className="mt-0.5"
                    />
                    <div className="flex-1">
                      <div className="font-medium">{opt.label}</div>
                      <p className="mt-0.5 text-xs text-neutral-400">
                        {opt.hint}
                      </p>
                    </div>
                  </label>
                </li>
              );
            })}
          </ul>
        </section>

        <section className="mb-6">
          <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
            Channels
          </h2>
          <div className="flex gap-2">
            {CHANNEL_OPTIONS.map((opt) => {
              const checked = channels.has(opt.value);
              return (
                <label
                  key={opt.value}
                  className={clsx(
                    "flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm",
                    checked
                      ? "border-sky-600 bg-sky-950/50 text-sky-200"
                      : "border-neutral-800 bg-neutral-900 text-neutral-300 hover:border-neutral-700",
                  )}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() => {
                      const next = new Set(channels);
                      if (checked) next.delete(opt.value);
                      else next.add(opt.value);
                      setChannels(next);
                    }}
                    disabled={running}
                  />
                  {opt.label}
                </label>
              );
            })}
          </div>
          <p className="mt-1 text-xs text-neutral-500">
            Interior/Rear are skipped for cameras that don&apos;t record
            them — single-channel dashcams only produce a Front output.
          </p>
        </section>

        <section className="mb-6">
          <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
            Scope
          </h2>
          <div className="flex flex-col gap-2">
            {SCOPE_OPTIONS.map((opt) => (
              <label
                key={opt.value}
                className="flex cursor-pointer items-start gap-3 rounded-md border border-neutral-800 bg-neutral-900 p-3 hover:border-neutral-700"
              >
                <input
                  type="radio"
                  name="tl-scope"
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
              {scanning ? (
                // Pre-encode scan phase: no count yet, just a heartbeat
                // bar so the user knows the worker is alive. Replaced
                // by the real progress UI when `timelapse:start` arrives.
                <>
                  <div className="mb-2 h-2 w-full overflow-hidden rounded-full bg-neutral-800">
                    <div className="h-full w-1/3 animate-pulse bg-sky-500/60" />
                  </div>
                  <div className="text-xs text-neutral-400">
                    Scanning library for new or changed trips…
                  </div>
                </>
              ) : (
                <>
                  <div className="mb-2 flex h-2 w-full overflow-hidden rounded-full bg-neutral-800">
                    <div
                      className="h-full bg-sky-500 transition-all"
                      style={{ width: `${donePct}%` }}
                    />
                    <div
                      className="h-full bg-red-500 transition-all"
                      style={{ width: `${failedPct}%` }}
                    />
                  </div>
                  <div className="flex items-center justify-between text-xs text-neutral-400">
                    <span>
                      {doneCount} / {total} jobs completed ({pct}%)
                    </span>
                    <div className="flex items-center gap-3">
                      {etaLabel && (
                        <span>
                          Batch ETA ≈{" "}
                          <span className="text-neutral-200">{etaLabel}</span>
                        </span>
                      )}
                      <span>{progress?.failed ?? 0} failed</span>
                      {running && startMs && <span>Elapsed {formatDurationShort(Math.max(0, now - startMs))}</span>}
                    </div>
                  </div>
                  {progress?.currentTripId && (
                    <div className="mt-1 truncate text-xs text-neutral-500">
                      trip {progress.currentTripId.slice(0, 8)}… · tier{" "}
                      {progress.currentTier} · channel {progress.currentChannel}
                    </div>
                  )}
                  {running && encodingProgress && (
                    <div className="mt-3 text-xs text-neutral-400">
                      <div className="flex flex-wrap justify-between gap-2">
                        <span>
                          {{ preparing: "Preparing source", encoding: "Encoding", finalizing: "Finalizing file" }[encodingProgress.phase]}
                          {" · "}{encoderLabel(encodingProgress.encoder)}
                          {" · "}{encodingProgress.tier} / {encodingProgress.channel}
                          {" · trip "}{encodingProgress.tripId.slice(0, 8)}…
                        </span>
                        {encodingEstimate?.remainingSeconds != null && (
                          <span>Current step ETA ≈ {formatDurationShort(encodingEstimate.remainingSeconds * 1000)}</span>
                        )}
                      </div>
                      {encodingEstimate && (
                        <>
                          <div className="my-2 h-2 overflow-hidden rounded-full bg-neutral-800">
                            <div className="h-full bg-sky-500 transition-all" style={{ width: `${encodingEstimate.percent}%` }} />
                          </div>
                          <span>{encodingEstimate.percent.toFixed(1)}% · video {formatDurationShort(encodingEstimate.processed * 1000)} / {formatDurationShort(encodingEstimate.total * 1000)}</span>
                          {encodingEstimate.stale && <span className="ml-2 text-amber-400">No recent FFmpeg progress update; it may still be busy.</span>}
                          {!encodingEstimate.stale && encodingEstimate.remainingSeconds == null && (
                            <span className="ml-2">{encodingEstimate.percent >= 100 ? "Finishing this step…" : "Waiting for measurable video progress…"}</span>
                          )}
                        </>
                      )}
                      <p className="mt-1">Step ETA covers the current phase of this channel. Batch ETA is approximate; CPU encoding can take longer than the original video.</p>
                    </div>
                  )}
                </>
              )}
            </div>
          </section>
        )}

        {lastResult && !running && (
          <section className="mb-6 rounded-md border border-neutral-800 bg-neutral-900 p-3 text-sm">
            <div className="font-medium">
              {lastResult.cancelled
                ? "Timelapse run cancelled"
                : "Timelapse run complete"}
            </div>
            <div className="mt-1 text-xs text-neutral-400">
              {lastResult.done} encoded ·{" "}
              <span
                className={
                  lastResult.failed > 0 ? "text-red-400" : undefined
                }
              >
                {lastResult.failed} failed
              </span>
            </div>
          </section>
        )}

        {trips.length > 0 && (
          <section className="mb-6">
            <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
              Overall coverage
            </h2>
            <div className="overflow-hidden rounded-md border border-neutral-800">
              <table className="w-full text-sm">
                <tbody>
                  {(["8x", "16x", "60x"] as const).map((tier) => {
                    const t = overallByTier[tier];
                    const tierOpt = TIER_OPTIONS.find((o) => o.value === tier);
                    const allCells: { state: CoverageState; count: number }[] = [
                      { state: "done", count: t.done },
                      { state: "running", count: t.running },
                      { state: "partial", count: t.partial },
                      { state: "failed", count: t.failed },
                      { state: "notRun", count: t.notRun },
                    ];
                    const cells = allCells.filter(
                      (c, i) => i === 0 || c.count > 0,
                    );
                    return (
                      <tr
                        key={tier}
                        className="border-t border-neutral-800 first:border-t-0"
                      >
                        <td className="px-3 py-2 text-neutral-200">
                          {tierOpt?.label.split(" — ")[0] ?? tier}
                        </td>
                        <td className="px-3 py-2">
                          <div className="flex flex-wrap items-center gap-1.5">
                            {cells.map(({ state, count }) => (
                              <span
                                key={state}
                                className={clsx(
                                  "inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium",
                                  COVERAGE_PILL_CLASSES[state],
                                )}
                              >
                                <span aria-hidden="true">
                                  {COVERAGE_PILL_GLYPH[state]}
                                </span>
                                {count} {COVERAGE_STATE_LABEL[state]}
                              </span>
                            ))}
                            <span className="ml-1 text-xs text-neutral-500">
                              of {trips.length} trips
                            </span>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <p className="mt-1 text-[11px] text-neutral-500">
              &quot;Playable&quot; means at least one channel completed at this
              tier — single-channel dashcams still count as playable once the
              Front output lands.
            </p>
          </section>
        )}

        {trips.length > 0 && (
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
                    <th className="px-3 py-2 text-left">Status</th>
                    <th className="px-3 py-2 text-left">Play</th>
                    <th className="px-3 py-2 text-left">Rebuild</th>
                    <th className="px-3 py-2 text-left"></th>
                  </tr>
                </thead>
                <tbody>
                  {trips.map((t) => {
                    const tripJobs = jobsByTrip[t.id] ?? [];
                    const doneCount = tripJobs.filter(
                      (j) => j.status === "done",
                    ).length;
                    const failedCount = tripJobs.filter(
                      (j) => j.status === "failed",
                    ).length;
                    const runningCount = tripJobs.filter(
                      (j) => j.status === "running",
                    ).length;
                    // Denominator for the "X/Y done" label excludes
                    // failed rows. A single-channel camera that was
                    // run with F+I+R selected leaves 6 permanent
                    // CameraDoesNotRecord failures; without this,
                    // those rows would inflate the denominator and
                    // the trip would read "3/9 done" forever despite
                    // being as complete as it can ever be. The
                    // failed count is still surfaced separately, so
                    // the user isn't misled — they just aren't
                    // punished for unavoidable failures.
                    const achievableTotal = tripJobs.length - failedCount;
                    // Max pad count across this trip's channels. We
                    // show a single badge per trip rather than per
                    // channel — users care about "does this trip have
                    // gaps" more than "which channel has them."
                    const maxPadded = tripJobs.reduce(
                      (m, j) => Math.max(m, j.paddedCount ?? 0),
                      0,
                    );
                    const doneTiers = new Set(
                      tripJobs
                        .filter((j) => j.status === "done")
                        .map((j) => j.tier),
                    );
                    return (
                      <tr
                        key={t.id}
                        className="border-t border-neutral-800 hover:bg-neutral-900/60"
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
                        <td className="px-3 py-2 text-xs">
                          {tripJobs.length === 0 && (
                            <span className="text-neutral-500">—</span>
                          )}
                          {tripJobs.length > 0 && (
                            <span className="flex gap-2">
                              {doneCount > 0 && (
                                <span className="text-emerald-400">
                                  {doneCount}/{achievableTotal} done
                                </span>
                              )}
                              {runningCount > 0 && (
                                <span className="text-sky-400">
                                  {runningCount} running
                                </span>
                              )}
                              {failedCount > 0 && (
                                <span className="text-red-400">
                                  {failedCount} failed
                                </span>
                              )}
                              {maxPadded > 0 && (
                                <span
                                  className="text-neutral-400"
                                  title={
                                    `A camera was off for ${maxPadded} segment(s) of ` +
                                    "this trip. The timelapse encodes the footage that " +
                                    "exists (fast GPU path, no penalty) and shows a black " +
                                    "overlay on that channel for those stretches."
                                  }
                                >
                                  {maxPadded} gap
                                  {maxPadded === 1 ? "" : "s"}
                                </span>
                              )}
                            </span>
                          )}
                          {failedCount > 0 && (
                            <details className="mt-2 max-w-lg text-neutral-400">
                              <summary className="cursor-pointer">Encoding error details</summary>
                              {tripJobs.filter((job) => job.status === "failed").map((job) => (
                                <p key={`${job.tier}-${job.channel}`} className="mt-1 whitespace-pre-wrap break-words">
                                  {job.tier} · {job.channel}: {job.errorMessage || "No diagnostic was recorded. Retry this job for details."}
                                </p>
                              ))}
                            </details>
                          )}
                        </td>
                        <td className="px-3 py-2">
                          <div className="flex gap-1">
                            {(["8x", "16x", "60x"] as const).map((tier) => {
                              const available = doneTiers.has(tier);
                              return (
                                <button
                                  key={tier}
                                  onClick={() => {
                                    // Load the trip's tier curve and
                                    // hand control back to the main
                                    // PlayerShell at that source. Trip
                                    // selection resets sourceMode to
                                    // "original", so we set it *after*
                                    // selecting the trip.
                                    const job = jobs.find(
                                      (j) =>
                                        j.tripId === t.id &&
                                        j.tier === tier &&
                                        j.status === "done" &&
                                        j.outputPath,
                                    );
                                    if (!job) return;
                                    const curve =
                                      parseCurveJson(
                                        job.speedCurveJson ?? null,
                                      ) ?? fallbackCurveForTier(t, tier);
                                    if (!curve) return;
                                    selectTrip(t.id);
                                    setSourceMode(tier, curve);
                                    setMainView("player");
                                  }}
                                  disabled={!available}
                                  className={clsx(
                                    "rounded px-2 py-1 text-xs font-medium transition-colors",
                                    available
                                      ? "bg-violet-700 text-white hover:bg-violet-600"
                                      : "cursor-not-allowed bg-neutral-800 text-neutral-600",
                                  )}
                                  title={
                                    available
                                      ? `Play ${tier} timelapse`
                                      : `No ${tier} timelapse encoded yet`
                                  }
                                >
                                  ▶ {tier}
                                </button>
                              );
                            })}
                          </div>
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
                                ? "Rebuild this trip with the selected tiers and channels (forces re-encode — scope is set to New & unfinished, which would otherwise skip done jobs)"
                                : `Rebuild this trip with the selected tiers and channels (scope: ${scope === "failedOnly" ? "Retry failed" : "Rebuild all"})`)
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
        {configured && (
          <section className="mb-6">
            <h2 className="mb-2 text-sm font-semibold uppercase tracking-wide text-neutral-400">
              Maintenance
            </h2>
            <div
              className={clsx(
                "rounded-md border p-3 text-sm",
                orphanCount > 0
                  ? "border-amber-700/60 bg-amber-950/30"
                  : "border-neutral-800 bg-neutral-900",
              )}
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex-1">
                  <div className="flex items-center gap-2">
                    <div className="font-medium text-neutral-200">
                      Prune orphan files
                    </div>
                    {orphanCount > 0 && (
                      <span
                        className="rounded-full bg-amber-600/30 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-amber-200"
                        title="Orphan timelapse files found on disk — click Prune to reclaim space."
                      >
                        {orphanCount} found
                      </span>
                    )}
                  </div>
                  <p className="mt-0.5 text-xs text-neutral-400">
                    Moves on-disk timelapse files whose trip_id matches no
                    database row to the OS trash. Reclaims space from
                    files left behind by trip renames or merges. Files go
                    to trash, not permanent deletion.
                  </p>
                </div>
                <button
                  onClick={() => void onPruneOrphans()}
                  disabled={
                    running || pruneState.kind === "running" || orphanCount === 0
                  }
                  className={clsx(
                    "shrink-0 rounded-md px-3 py-1.5 text-sm font-medium",
                    running || pruneState.kind === "running" || orphanCount === 0
                      ? "cursor-not-allowed bg-neutral-800 text-neutral-500"
                      : orphanCount > 0
                        ? "bg-amber-700 text-white hover:bg-amber-600"
                        : "bg-neutral-700 text-neutral-100 hover:bg-neutral-600",
                  )}
                  title={
                    running
                      ? "Wait for the current encode to finish first"
                      : orphanCount === 0
                        ? "No orphan files to prune"
                        : undefined
                  }
                >
                  {pruneState.kind === "running" ? "Pruning…" : "Prune"}
                </button>
              </div>
              {pruneState.kind === "done" && (
                <div className="mt-2 rounded-md bg-emerald-950/60 px-3 py-2 text-xs text-emerald-200">
                  Moved <strong>{pruneState.trashed}</strong>{" "}
                  {pruneState.trashed === 1 ? "file" : "files"} to trash
                  {pruneState.bytes > 0 && (
                    <>
                      {" "}
                      · reclaimed{" "}
                      <strong>{formatBytes(pruneState.bytes)}</strong>
                    </>
                  )}
                  {pruneState.trashed === 0 && (
                    <> — no orphans to clean up</>
                  )}
                  .
                </div>
              )}
              {pruneState.kind === "error" && (
                <div className="mt-2 rounded-md bg-red-950/60 px-3 py-2 text-xs text-red-200">
                  Prune failed: {pruneState.message}
                </div>
              )}
            </div>
          </section>
        )}
      </div>

      <footer className="flex items-center justify-end gap-2 border-t border-neutral-800 px-4 py-3">
        {running ? (
          <button
            onClick={() => void cancelRun()}
            className="rounded-md bg-red-700 px-4 py-2 text-sm font-medium text-white hover:bg-red-600"
          >
            Cancel
          </button>
        ) : (
          <button
            onClick={() => void onStart()}
            disabled={!canStart}
            className={clsx(
              "inline-flex items-center gap-2 rounded-md px-4 py-2 text-sm font-medium",
              !canStart
                ? "cursor-not-allowed bg-neutral-800 text-neutral-500"
                : "bg-sky-600 text-white hover:bg-sky-500",
            )}
            title={
              !configured
                ? "Configure ffmpeg first"
                : tiers.size === 0
                  ? "Pick at least one tier"
                  : channels.size === 0
                    ? "Pick at least one channel"
                    : undefined
            }
          >
            Start encoding
          </button>
        )}
      </footer>

      {showConfig && <FfmpegConfig onClose={() => setShowConfig(false)} />}
    </div>
  );
}
