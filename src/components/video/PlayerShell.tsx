import { useCallback, useEffect, useRef } from "react";
import { useSyncEngine } from "../../engine/useSyncEngine";
import { useStore } from "../../state/store";
import type { Segment } from "../../types/model";
import { KeyboardShortcuts } from "../controls/KeyboardShortcuts";
import { TransportControls } from "../controls/TransportControls";
import { DriftHud } from "../hud/DriftHud";
import { MapPanel } from "../map/MapPanel";
import { Timeline } from "../timeline/Timeline";
import { VideoGrid } from "./VideoGrid";

export function PlayerShell() {
  const frontRef = useRef<HTMLVideoElement>(null);
  const interiorRef = useRef<HTMLVideoElement>(null);
  const rearRef = useRef<HTMLVideoElement>(null);
  const shouldAutoPlay = useRef(false);
  const pendingSeekRef = useRef<number | null>(null);

  const activeSegment = useStore((s): Segment | null => {
    const trip = s.trips.find((t) => t.id === s.loadedTripId);
    if (!trip || trip.segments.length === 0) return null;
    if (s.activeSegmentId) {
      const seg = trip.segments.find((x) => x.id === s.activeSegmentId);
      if (seg) return seg;
    }
    return trip.segments[0];
  });

  const engine = useSyncEngine(
    frontRef,
    interiorRef,
    rearRef,
    activeSegment?.id ?? null,
  );

  // Segment auto-advance on ended
  useEffect(() => {
    const front = frontRef.current;
    if (!front) return;

    const onEnded = () => {
      const { trips, loadedTripId, activeSegmentId } = useStore.getState();
      const trip = trips.find((t) => t.id === loadedTripId);
      if (!trip) return;

      const currentId = activeSegmentId ?? trip.segments[0]?.id;
      const idx = trip.segments.findIndex((s) => s.id === currentId);
      const next = trip.segments[idx + 1];

      if (next) {
        shouldAutoPlay.current = true;
        useStore.getState().setActiveSegmentId(next.id);
      } else {
        useStore.getState().setIsPlaying(false);
      }
    };

    front.addEventListener("ended", onEnded);
    return () => front.removeEventListener("ended", onEnded);
  }, [activeSegment?.id]);

  // Auto-play after segment advance or cross-segment seek
  useEffect(() => {
    if (!engine) return;
    if (shouldAutoPlay.current) {
      shouldAutoPlay.current = false;
      void engine.play();
    }
    if (pendingSeekRef.current !== null) {
      engine.seek(pendingSeekRef.current);
      pendingSeekRef.current = null;
    }
  }, [engine]);

  // Seek to an arbitrary trip-level time (may cross segment boundaries)
  const seekToTripTime = useCallback(
    (tripTime: number) => {
      const { trips, loadedTripId, activeSegmentId, isPlaying } =
        useStore.getState();
      const trip = trips.find((t) => t.id === loadedTripId);
      if (!trip) return;

      let cumulative = 0;
      for (const seg of trip.segments) {
        if (tripTime < cumulative + seg.durationS) {
          const timeInSeg = tripTime - cumulative;
          const currentSegId = activeSegmentId ?? trip.segments[0]?.id;

          if (seg.id === currentSegId) {
            engine?.seek(timeInSeg);
          } else {
            pendingSeekRef.current = timeInSeg;
            if (isPlaying) shouldAutoPlay.current = true;
            useStore.setState({ activeSegmentId: seg.id, currentTime: 0 });
          }
          return;
        }
        cumulative += seg.durationS;
      }

      // Past the end — seek to end of last segment
      const last = trip.segments[trip.segments.length - 1];
      if (last) {
        const currentSegId = activeSegmentId ?? trip.segments[0]?.id;
        if (last.id === currentSegId) {
          engine?.seek(last.durationS);
        } else {
          pendingSeekRef.current = last.durationS;
          if (useStore.getState().isPlaying) shouldAutoPlay.current = true;
          useStore.setState({ activeSegmentId: last.id, currentTime: 0 });
        }
      }
    },
    [engine],
  );

  return (
    <div className="flex h-full flex-col">
      <div className="relative grid min-h-0 flex-1 grid-cols-[2fr_1fr_1fr] gap-2 p-2">
        <VideoGrid
          frontRef={frontRef}
          interiorRef={interiorRef}
          rearRef={rearRef}
          activeSegment={activeSegment}
        />
        <MapPanel activeSegment={activeSegment} />
        <DriftHud />
      </div>
      <div className="border-t border-neutral-800 bg-neutral-950 px-4 pt-2">
        <Timeline onSeekTripTime={seekToTripTime} />
      </div>
      <TransportControls engine={engine} />
      <KeyboardShortcuts engine={engine} />
    </div>
  );
}
