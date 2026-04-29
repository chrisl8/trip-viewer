import { useMemo } from "react";
import type { SyncEngine } from "../../engine/SyncEngine";
import type { PlaybackSlice } from "../../state/store";
import { useStore } from "../../state/store";
import { computeTripTime, tripTotalDuration } from "../../utils/tripTime";
import { SourceControls, type SourceOption } from "./SourceControls";
import { SpeedControls } from "./SpeedControls";

type SourceMode = PlaybackSlice["sourceMode"];

interface Props {
  engine: SyncEngine | null;
  /** Invoked when the user picks a different source. PlayerShell owns
   *  the trip-time preservation + curve load + video-seek coordination;
   *  this component just surfaces the pick. */
  onSourceChange: (mode: SourceMode) => void;
}

function formatTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const mins = Math.floor(s / 60);
  const secs = Math.floor(s % 60);
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

/** Trip-local status of a tier's encode jobs, used for the source picker. */
function tierAvailability(
  jobs: ReturnType<typeof useStore.getState>["timelapseJobs"],
  tripId: string | null,
  tier: "8x" | "16x" | "60x",
): SourceOption {
  if (!tripId) {
    return {
      mode: tier,
      enabled: false,
      disabledReason: "Load a trip first",
    };
  }
  const forTrip = jobs.filter((j) => j.tripId === tripId && j.tier === tier);
  if (forTrip.length === 0) {
    return {
      mode: tier,
      enabled: false,
      disabledReason: `${tier} not yet encoded for this trip`,
    };
  }
  const running = forTrip.some((j) => j.status === "running");
  const done = forTrip.filter((j) => j.status === "done");
  if (done.length === 0 && running) {
    return {
      mode: tier,
      enabled: false,
      disabledReason: `${tier} encoding in progress`,
    };
  }
  if (done.length === 0) {
    return {
      mode: tier,
      enabled: false,
      disabledReason: `${tier} not yet encoded for this trip`,
    };
  }
  // At least one channel done — tier is playable (partial-tier
  // availability shows missing channels as a placeholder in VideoGrid).
  return { mode: tier, enabled: true };
}

export function TransportControls({ engine, onSourceChange }: Props) {
  const isPlaying = useStore((s) => s.isPlaying);
  const currentTime = useStore((s) => s.currentTime);
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const speed = useStore((s) => s.speed);
  const sourceMode = useStore((s) => s.sourceMode);
  const activeSpeedCurve = useStore((s) => s.activeSpeedCurve);
  const timelapseJobs = useStore((s) => s.timelapseJobs);
  const disabled = !engine;

  const trip = trips.find((t) => t.id === loadedTripId);
  const tripTime = computeTripTime(
    trip,
    activeSegmentId,
    currentTime,
    sourceMode,
    activeSpeedCurve,
  );
  const totalDuration = tripTotalDuration(trip);

  const sourceOptions: SourceOption[] = useMemo(
    () => {
      // Archive-only trips have no source segments left on disk, so the
      // Original tier can't play. Show it as disabled with a clear reason
      // rather than hiding it — keeps the picker layout stable across
      // trips and tells the user *why* it's unavailable.
      const archive = trip?.archiveOnly === true;
      const originalOption: SourceOption = archive
        ? {
            mode: "original",
            enabled: false,
            disabledReason:
              "Source files have been deleted. Only the timelapse archive is available.",
          }
        : { mode: "original", enabled: Boolean(trip) };
      return [
        originalOption,
        tierAvailability(timelapseJobs, loadedTripId, "8x"),
        tierAvailability(timelapseJobs, loadedTripId, "16x"),
        tierAvailability(timelapseJobs, loadedTripId, "60x"),
      ];
    },
    [trip, timelapseJobs, loadedTripId],
  );

  const onToggle = () => {
    if (!engine) return;
    if (isPlaying) engine.pause();
    else void engine.play();
  };

  // Effective playback rate = source-tier × speed. Always shown so
  // the user can see the composition every time, including 1× ×1 in
  // Original mode (which reads as "1× effective" — boring but it
  // makes the multiplicative model unmistakable).
  const tierRate =
    sourceMode === "8x" ? 8 : sourceMode === "16x" ? 16 : sourceMode === "60x" ? 60 : 1;
  const effectiveRate = tierRate * speed;

  return (
    <div className="flex flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-4 py-2 sm:gap-4">
      <button
        onClick={onToggle}
        disabled={disabled}
        className="w-20 shrink-0 rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {isPlaying ? "Pause" : "Play"}
      </button>

      <SourceControls
        current={sourceMode}
        options={sourceOptions}
        onChange={onSourceChange}
        disabled={disabled}
      />

      <SpeedControls engine={engine} />

      <span className="shrink-0 text-[11px] text-neutral-500">
        →{" "}
        <span className="text-neutral-300">{effectiveRate}×</span> effective
      </span>

      <div className="ml-auto shrink-0 font-mono text-xs tabular-nums text-neutral-400">
        {formatTime(tripTime)} / {formatTime(totalDuration)}
      </div>
    </div>
  );
}
