import { useCallback, useEffect, useMemo, useRef } from "react";
import { useSyncEngine } from "../../engine/useSyncEngine";
import { useStore } from "../../state/store";
import type { Segment } from "../../types/model";
import { KeyboardShortcuts } from "../controls/KeyboardShortcuts";
import { TransportControls } from "../controls/TransportControls";
import { DriftHud } from "../hud/DriftHud";
import { MapPanel } from "../map/MapPanel";
import { SegmentTagBar } from "../review/SegmentTagBar";
import { Timeline } from "../timeline/Timeline";
import { VideoGrid } from "./VideoGrid";

export function PlayerShell() {
  // Single map of label → <video> element, populated by callback refs in
  // VideoGrid. Stable across renders so useSyncEngine's deps array doesn't
  // churn. Keyed by channel label so it works for any channel count.
  const channelRefs = useRef<Map<string, HTMLVideoElement | null>>(new Map());
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

  // Ordered list of channel labels for the current segment (canonical order).
  // useMemo so the identity is stable as long as the labels are.
  const channelLabels = useMemo(
    () => activeSegment?.channels.map((c) => c.label) ?? [],
    [activeSegment],
  );

  const engine = useSyncEngine(channelRefs, channelLabels, activeSegment?.id ?? null);

  // Segment auto-advance on ended. The master channel (first in canonical
  // order, i.e. channels[0]) drives this.
  useEffect(() => {
    if (!activeSegment) return;
    const masterLabel = activeSegment.channels[0]?.label;
    if (!masterLabel) return;
    const master = channelRefs.current.get(masterLabel);
    if (!master) return;

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

    master.addEventListener("ended", onEnded);
    return () => master.removeEventListener("ended", onEnded);
  }, [activeSegment]);

  // Auto-play after segment advance or cross-segment seek.
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

  // Seek to an arbitrary trip-level time (may cross segment boundaries).
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

      // Past the end — seek to end of last segment.
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

  // When the active segment's camera doesn't record GPS, collapse the map
  // slot and let the video grid grow into the freed space. A small muted
  // caption explains why — so users aren't left wondering where the map went.
  const gpsSupported = activeSegment?.gpsSupported ?? true;
  const gridCols = gpsSupported
    ? "grid-cols-[2fr_1fr_1fr]"
    : "grid-cols-[3fr_1fr]";

  return (
    <div className="flex h-full flex-col">
      <div className={`relative grid min-h-0 flex-1 ${gridCols} gap-2 p-2`}>
        <VideoGrid channelRefs={channelRefs} activeSegment={activeSegment} />
        {gpsSupported && <MapPanel activeSegment={activeSegment} />}
        <DriftHud />
      </div>
      {!gpsSupported && activeSegment && (
        <div className="border-t border-neutral-800 bg-neutral-950 px-4 py-1 text-xs text-neutral-500">
          This camera model doesn&rsquo;t record GPS data.
        </div>
      )}
      <div className="border-t border-neutral-800 bg-neutral-950">
        {activeSegment && <SegmentTagBar segment={activeSegment} />}
        <div className="px-4 pt-1">
          <Timeline onSeekTripTime={seekToTripTime} />
        </div>
      </div>
      <TransportControls engine={engine} />
      <KeyboardShortcuts engine={engine} />
    </div>
  );
}
