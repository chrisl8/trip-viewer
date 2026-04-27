import { useCallback, useEffect, useMemo, useRef } from "react";
import { useSyncEngine } from "../../engine/useSyncEngine";
import { useStore } from "../../state/store";
import type { Channel, Segment } from "../../types/model";
import {
  LABEL_FRONT,
  LABEL_INTERIOR,
  LABEL_REAR,
} from "../../types/model";
import {
  concatToFile,
  parseCurveJson,
  type CurveSegment,
} from "../../utils/speedCurve";
import {
  activeSegmentAtConcatTime,
  computeTripTime,
  seekTripTime,
} from "../../utils/tripTime";
import { KeyboardShortcuts } from "../controls/KeyboardShortcuts";
import { TransportControls } from "../controls/TransportControls";
import { DriftHud } from "../hud/DriftHud";
import { MapPanel } from "../map/MapPanel";
import { SegmentTagBar } from "../review/SegmentTagBar";
import { Timeline } from "../timeline/Timeline";
import { VideoGrid } from "./VideoGrid";

/** Map the backend's F/I/R channel code to the frontend's canonical
 *  label. Tier synthesis uses this to build Channel objects whose
 *  labels match the existing UI (primaryChannel, SegmentTagBar). */
function channelLabelFromCode(code: string): string {
  switch (code) {
    case "F":
      return LABEL_FRONT;
    case "I":
      return LABEL_INTERIOR;
    case "R":
      return LABEL_REAR;
    default:
      return code;
  }
}

export function PlayerShell() {
  // Single map of label → <video> element, populated by callback refs in
  // VideoGrid. Stable across renders so useSyncEngine's deps array doesn't
  // churn. Keyed by channel label so it works for any channel count.
  const channelRefs = useRef<Map<string, HTMLVideoElement | null>>(new Map());
  const shouldAutoPlay = useRef(false);
  const pendingSeekRef = useRef<number | null>(null);

  const sourceMode = useStore((s) => s.sourceMode);
  const activeSpeedCurve = useStore((s) => s.activeSpeedCurve);
  const timelapseJobs = useStore((s) => s.timelapseJobs);

  // The "real" current segment — from the trip's segment list, based
  // on store.activeSegmentId. Drives SegmentTagBar and MapPanel in
  // Original mode, and continues to drive them in tiered mode where
  // we update activeSegmentId as the playhead crosses virtual segment
  // boundaries (so tags + map stay on the right segment).
  const activeSegmentForUi = useStore((s): Segment | null => {
    const trip = s.trips.find((t) => t.id === s.loadedTripId);
    if (!trip || trip.segments.length === 0) return null;
    if (s.activeSegmentId) {
      const seg = trip.segments.find((x) => x.id === s.activeSegmentId);
      if (seg) return seg;
    }
    return trip.segments[0];
  });

  const trip = useStore((s) => s.trips.find((t) => t.id === s.loadedTripId));

  // In tiered mode we feed VideoGrid (and useSyncEngine) a synthetic
  // Segment whose id is stable across virtual-segment boundaries.
  // Keeps the engine from re-initializing every time the playhead
  // crosses into a new virtual segment — the underlying MP4 files
  // are the same. activeSegmentForUi still moves independently to
  // drive tags / timeline highlights.
  const activeSegmentForVideo = useMemo((): Segment | null => {
    if (sourceMode === "original") return activeSegmentForUi;
    if (!trip) return null;

    const tier = sourceMode; // "8x" | "16x" | "60x"
    const jobs = timelapseJobs.filter(
      (j) =>
        j.tripId === trip.id &&
        j.tier === tier &&
        j.status === "done" &&
        j.outputPath,
    );
    if (jobs.length === 0) return null;

    // Build channels in F → I → R order regardless of job-row order.
    const order = ["F", "I", "R"];
    const ordered = [...jobs].sort(
      (a, b) => order.indexOf(a.channel) - order.indexOf(b.channel),
    );
    const channels: Channel[] = ordered.map((j) => ({
      label: channelLabelFromCode(j.channel),
      filePath: j.outputPath as string,
      width: null,
      height: null,
      fpsNum: null,
      fpsDen: null,
      codec: null,
      hasGpmdTrack: false,
    }));

    return {
      id: `__tier_${tier}_${trip.id}`,
      startTime: trip.startTime,
      durationS: trip.segments.reduce((sum, s) => sum + s.durationS, 0),
      isEvent: false,
      channels,
      cameraKind: trip.segments[0]?.cameraKind ?? "wolfBox",
      gpsSupported: trip.segments[0]?.gpsSupported ?? true,
    };
  }, [sourceMode, activeSegmentForUi, trip, timelapseJobs]);

  // Ordered list of channel labels for the current engine lineup.
  const channelLabels = useMemo(
    () => activeSegmentForVideo?.channels.map((c) => c.label) ?? [],
    [activeSegmentForVideo],
  );

  const engine = useSyncEngine(
    channelRefs,
    channelLabels,
    activeSegmentForVideo?.id ?? null,
  );

  // Segment auto-advance on ended. Only relevant in Original mode —
  // in tiered mode there's a single file spanning the whole trip, so
  // ending just means playback is complete.
  useEffect(() => {
    if (!activeSegmentForVideo) return;
    if (sourceMode !== "original") {
      // Tiered mode still gets an "ended" event (the single file
      // finished). Stop playback but don't try to advance segments.
      const masterLabel = activeSegmentForVideo.channels[0]?.label;
      if (!masterLabel) return;
      const master = channelRefs.current.get(masterLabel);
      if (!master) return;
      const onEnded = () => useStore.getState().setIsPlaying(false);
      master.addEventListener("ended", onEnded);
      return () => master.removeEventListener("ended", onEnded);
    }

    const masterLabel = activeSegmentForVideo.channels[0]?.label;
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
  }, [activeSegmentForVideo, sourceMode]);

  // Auto-play after segment advance, cross-segment seek, or source switch.
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

  // In tiered mode the engine's tick writes file-time to
  // store.currentTime. We derive the virtual active segment from
  // that current concat-time and update activeSegmentId if it moved.
  // Throttled naturally by the engine's tick rate; this effect is
  // cheap (a linear walk of segments).
  const currentTime = useStore((s) => s.currentTime);
  useEffect(() => {
    if (sourceMode === "original" || !trip || !activeSpeedCurve) return;
    const concatT = computeTripTime(
      trip,
      null,
      currentTime,
      sourceMode,
      activeSpeedCurve,
    );
    const virtual = activeSegmentAtConcatTime(trip, concatT);
    if (virtual && virtual !== useStore.getState().activeSegmentId) {
      // Update activeSegmentId WITHOUT going through setActiveSegmentId
      // (which would reset currentTime and primaryChannel — we don't
      // want either in tiered mode; currentTime is the tier file-time,
      // not segment-local, and the channel list is stable).
      useStore.setState({ activeSegmentId: virtual });
    }
  }, [sourceMode, activeSpeedCurve, trip, currentTime]);

  // Seek to an arbitrary trip-level time (may cross segment boundaries
  // in Original mode; is a single file-seek in tiered mode).
  const seekToTripTime = useCallback(
    (tripTime: number) => {
      const { trips, loadedTripId, activeSegmentId, isPlaying, sourceMode, activeSpeedCurve } =
        useStore.getState();
      const trip = trips.find((t) => t.id === loadedTripId);
      if (!trip) return;

      const target = seekTripTime(tripTime, trip, sourceMode, activeSpeedCurve);
      if (!target) return;

      if (target.mode === "original") {
        const currentSegId = activeSegmentId ?? trip.segments[0]?.id;
        if (target.activeSegmentId === currentSegId) {
          engine?.seek(target.segmentLocalTime);
        } else {
          pendingSeekRef.current = target.segmentLocalTime;
          if (isPlaying) shouldAutoPlay.current = true;
          useStore.setState({
            activeSegmentId: target.activeSegmentId,
            currentTime: 0,
          });
        }
      } else {
        // Tiered: single-file seek. activeSegmentId tracks the virtual
        // current segment for tags; useEffect above will also fire on
        // the currentTime change, so this write + engine.seek keeps
        // everything consistent.
        engine?.seek(target.fileTime);
        if (target.virtualActiveSegmentId) {
          useStore.setState({ activeSegmentId: target.virtualActiveSegmentId });
        }
      }
    },
    [engine],
  );

  /**
   * Swap playback source. Preserves trip-time and play state: we
   * compute the current concat-time in the old mode, load the new
   * mode's curve (if tiered), set the store flags, and queue a
   * pending seek in the new mode's time axis so the reloaded engine
   * lands at the equivalent moment.
   */
  const onSourceChange = useCallback(
    (newMode: ReturnType<typeof useStore.getState>["sourceMode"]) => {
      const state = useStore.getState();
      const oldMode = state.sourceMode;
      if (newMode === oldMode) return;

      const trip = state.trips.find((t) => t.id === state.loadedTripId);
      if (!trip) return;

      // 1. Snapshot current trip-time in the old mode.
      const tripTime = computeTripTime(
        trip,
        state.activeSegmentId,
        state.currentTime,
        oldMode,
        state.activeSpeedCurve,
      );

      // 2. Resolve the new mode's curve (tier) or clear it (Original).
      let newCurve: CurveSegment[] | null = null;
      if (newMode !== "original") {
        // Pull any done job's curve for (trip, tier). All three
        // channels of a given tier share the same curve by design.
        const job = state.timelapseJobs.find(
          (j) =>
            j.tripId === trip.id &&
            j.tier === newMode &&
            j.status === "done" &&
            j.speedCurveJson,
        );
        newCurve = parseCurveJson(job?.speedCurveJson ?? null);
        if (!newCurve) {
          // Legacy row (pre-curve column) — can't play tiered safely.
          // User will rebuild-all after this change ships, so this
          // fallback path shouldn't hit in normal use.
          console.warn(
            `[player] ${newMode} has no speed curve for trip ${trip.id}; staying on ${oldMode}`,
          );
          return;
        }
      }

      // 3. Compute the target in the new mode.
      if (newMode === "original") {
        // Find (segment, local-time) for tripTime.
        let cumulative = 0;
        let targetSegId = trip.segments[0]?.id ?? null;
        let targetLocal = 0;
        for (const seg of trip.segments) {
          if (tripTime < cumulative + seg.durationS) {
            targetSegId = seg.id;
            targetLocal = tripTime - cumulative;
            break;
          }
          cumulative += seg.durationS;
          targetSegId = seg.id;
          targetLocal = seg.durationS;
        }
        // 4. Queue a pending seek in segment-local time so the
        //    engine-recreated-for-new-segment picks it up on mount.
        pendingSeekRef.current = targetLocal;
        if (state.isPlaying) shouldAutoPlay.current = true;
        useStore.setState({
          sourceMode: "original",
          activeSpeedCurve: null,
          activeSegmentId: targetSegId,
          // currentTime is segment-local in Original; set to 0 so it
          // doesn't momentarily appear out-of-range before pendingSeek.
          currentTime: 0,
        });
      } else {
        // Tiered: target is a file-time derived from tripTime via
        // the new curve.
        const fileTime = concatToFile(tripTime, newCurve!);
        pendingSeekRef.current = fileTime;
        if (state.isPlaying) shouldAutoPlay.current = true;
        const virtualSeg = activeSegmentAtConcatTime(trip, tripTime);
        useStore.setState({
          sourceMode: newMode,
          activeSpeedCurve: newCurve,
          activeSegmentId: virtualSeg,
          // currentTime is file-time in tiered; it'll get overwritten
          // by the engine tick once the video loads, but start from 0
          // to avoid showing a stale segment-local value.
          currentTime: 0,
        });
      }
    },
    [],
  );

  // When the active segment's camera doesn't record GPS, collapse the map
  // slot and let the video grid grow into the freed space. A small muted
  // caption explains why — so users aren't left wondering where the map went.
  const gpsSupported = activeSegmentForUi?.gpsSupported ?? true;
  const gridCols = gpsSupported
    ? "grid-cols-[2fr_1fr_1fr]"
    : "grid-cols-[3fr_1fr]";

  return (
    <div className="flex h-full flex-col">
      <div className={`relative grid min-h-0 flex-1 ${gridCols} gap-2 p-2`}>
        <VideoGrid
          channelRefs={channelRefs}
          activeSegment={activeSegmentForVideo}
        />
        {gpsSupported && <MapPanel activeSegment={activeSegmentForUi} />}
        <DriftHud />
      </div>
      {!gpsSupported && activeSegmentForUi && (
        <div className="border-t border-neutral-800 bg-neutral-950 px-4 py-1 text-xs text-neutral-500">
          This camera model doesn&rsquo;t record GPS data.
        </div>
      )}
      <div className="border-t border-neutral-800 bg-neutral-950">
        {activeSegmentForUi && <SegmentTagBar segment={activeSegmentForUi} />}
        <div className="px-4 pt-1">
          <Timeline onSeekTripTime={seekToTripTime} />
        </div>
      </div>
      <TransportControls engine={engine} onSourceChange={onSourceChange} />
      <KeyboardShortcuts engine={engine} />
    </div>
  );
}
