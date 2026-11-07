import { useEffect, useRef, useState } from "react";
import { SyncEngine } from "./SyncEngine";
import type { CurveSegment } from "../utils/speedCurve";

/**
 * Wire up a `SyncEngine` instance for the current segment.
 *
 * @param channelRefs  Map keyed by channel label; populated by `VideoGrid`
 *                     as each `<video>` element mounts.
 * @param channelLabels Ordered list of labels in the current segment
 *                     (canonical order — first entry is the sync master).
 * @param activeSegmentId The current segment id (changing this recreates the engine).
 * @param channelCurves Per-channel speed curves keyed by label, for tiered
 *                     playback. The master's curve maps file-time → concat-time;
 *                     a slave's curve drives its coverage-gap hold/overlay.
 *                     Empty map in Original mode. Captured at engine
 *                     construction (stable per tier, so not a dep).
 */
export function useSyncEngine(
  channelRefs: React.MutableRefObject<Map<string, HTMLVideoElement | null>>,
  channelLabels: string[],
  activeSegmentId: string | null,
  channelCurves: Map<string, CurveSegment[]> = new Map(),
): SyncEngine | null {
  const [engine, setEngine] = useState<SyncEngine | null>(null);
  const engineRef = useRef<SyncEngine | null>(null);

  // Stable string key that captures the identity of the current segment's
  // channel lineup. If either the segment id OR the set of channel labels
  // changes, we tear down the engine and rebuild with the new lineup.
  const labelsKey = channelLabels.join("|");

  useEffect(() => {
    engineRef.current?.pause();
    engineRef.current?.dispose();
    engineRef.current = null;
    setEngine(null);

    if (!activeSegmentId || channelLabels.length === 0) return;

    const masterLabel = channelLabels[0];
    const slaveLabels = channelLabels.slice(1);

    const getEl = (label: string) => channelRefs.current.get(label) ?? null;

    const tryInit = () => {
      const master = getEl(masterLabel);
      if (!master || master.readyState < 2) return;
      if (engineRef.current) return;

      // Wait for every slave to be ready — if we init with a partial set,
      // the engineRef guard prevents re-initialization and the missing
      // slaves are permanently excluded from control (observable as those
      // channels freezing after scrubbing).
      const slaves: HTMLVideoElement[] = [];
      const includedLabels: string[] = [];
      for (const label of slaveLabels) {
        const el = getEl(label);
        if (!el || el.readyState < 2) return;
        slaves.push(el);
        includedLabels.push(label);
      }

      const masterCurve = channelCurves.get(masterLabel) ?? null;
      const slaveCurves = includedLabels.map((l) => channelCurves.get(l) ?? null);
      const e = new SyncEngine(master, slaves, includedLabels, masterCurve, slaveCurves);
      e.start();
      engineRef.current = e;
      setEngine(e);
      window.clearInterval(readinessPoll);
    };

    // WebKit can reach HAVE_CURRENT_DATA without delivering loadeddata after
    // a source reset. Readiness must not leave Play disabled forever merely
    // because that event was lost. Polling only reads state; it never seeks.
    const readinessPoll = window.setInterval(tryInit, 250);
    tryInit();

    // Re-try every time any channel fires `loadeddata`. We listen on all
    // channels because the engine won't init until the last slow channel
    // is ready.
    const allLabels = [masterLabel, ...slaveLabels];
    const listeners: Array<[HTMLVideoElement, () => void]> = [];
    for (const label of allLabels) {
      const el = getEl(label);
      if (!el) continue;
      const h = () => tryInit();
      el.addEventListener("loadeddata", h);
      listeners.push([el, h]);
    }

    return () => {
      window.clearInterval(readinessPoll);
      for (const [el, h] of listeners) el.removeEventListener("loadeddata", h);
      engineRef.current?.pause();
      engineRef.current?.dispose();
      engineRef.current = null;
      setEngine(null);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSegmentId, labelsKey, channelRefs]);

  return engine;
}
