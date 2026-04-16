import { useEffect, useRef, useState } from "react";
import { SyncEngine } from "./SyncEngine";

// Windows/macOS always render all channels of a segment; Linux has an
// opt-in single-channel mode where non-master refs may be null or not-ready.
// This IS_LINUX is scoped to that partial-slave tolerance only — it is
// intentionally separate from SyncEngine.ts's SKIP_DRIFT_CORRECTION
// (which also covers macOS) because the two concerns are orthogonal:
// macOS wants full three-channel playback like Windows, but needs the
// drift-correction skip because WKWebView shares WebKit's pipeline-flush
// semantics. VideoGrid.tsx's IS_LINUX is Linux-only for a third reason
// (the asset-protocol workaround / single-channel layout).
const IS_LINUX =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Linux") &&
  !navigator.userAgent.includes("Android");

/**
 * Wire up a `SyncEngine` instance for the current segment.
 *
 * @param channelRefs  Map keyed by channel label; populated by `VideoGrid`
 *                     as each `<video>` element mounts.
 * @param channelLabels Ordered list of labels in the current segment
 *                     (canonical order — first entry is the sync master).
 * @param activeSegmentId The current segment id (changing this recreates the engine).
 */
export function useSyncEngine(
  channelRefs: React.MutableRefObject<Map<string, HTMLVideoElement | null>>,
  channelLabels: string[],
  activeSegmentId: string | null,
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

      // On Windows/macOS, wait for every slave to be ready — if we init
      // with a partial set, the engineRef guard prevents re-initialization
      // and the missing slaves are permanently excluded from control
      // (observable as those channels freezing after scrubbing).
      //
      // On Linux, multi-channel is opt-in and slaves may legitimately be
      // null in single-channel mode. SyncEngine's tick loop and play/
      // pause/seek/setSpeed iterate `this.slaves` — an empty subset is
      // a safe no-op.
      const slaves: HTMLVideoElement[] = [];
      const includedLabels: string[] = [];
      for (const label of slaveLabels) {
        const el = getEl(label);
        if (el && el.readyState >= 2) {
          slaves.push(el);
          includedLabels.push(label);
        } else if (!IS_LINUX) {
          return; // Windows/macOS: all slaves must be ready.
        }
      }

      const e = new SyncEngine(master, slaves, includedLabels);
      e.start();
      engineRef.current = e;
      setEngine(e);
    };

    tryInit();

    // Re-try every time any channel fires `loadeddata`. We listen on all
    // channels (master + slaves) because any of them flipping to
    // readyState >= 2 might make the engine viable on Linux, and on
    // Windows the engine won't init until the last slow channel is ready.
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
