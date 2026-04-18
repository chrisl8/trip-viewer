import type { SyncEngine } from "../../engine/SyncEngine";
import { useStore } from "../../state/store";
import { SpeedControls } from "./SpeedControls";

interface Props {
  engine: SyncEngine | null;
}

function formatTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const mins = Math.floor(s / 60);
  const secs = Math.floor(s % 60);
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

export function TransportControls({ engine }: Props) {
  const isPlaying = useStore((s) => s.isPlaying);
  const currentTime = useStore((s) => s.currentTime);
  const trips = useStore((s) => s.trips);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const disabled = !engine;

  const trip = trips.find((t) => t.id === loadedTripId);
  let tripTime = 0;
  let totalDuration = 0;
  if (trip) {
    const activeId = activeSegmentId ?? trip.segments[0]?.id;
    for (const seg of trip.segments) {
      if (seg.id === activeId) {
        tripTime = totalDuration + currentTime;
      }
      totalDuration += seg.durationS;
    }
  }

  const onToggle = () => {
    if (!engine) return;
    if (isPlaying) engine.pause();
    else void engine.play();
  };

  return (
    <div className="flex flex-wrap items-center gap-2 border-t border-neutral-800 bg-neutral-950 px-4 py-2 sm:gap-4">
      <button
        onClick={onToggle}
        disabled={disabled}
        className="w-20 shrink-0 rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {isPlaying ? "Pause" : "Play"}
      </button>

      <SpeedControls engine={engine} />

      <div className="ml-auto shrink-0 font-mono text-xs tabular-nums text-neutral-400">
        {formatTime(tripTime)} / {formatTime(totalDuration)}
      </div>
    </div>
  );
}
