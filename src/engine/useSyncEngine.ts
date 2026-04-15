import { RefObject, useEffect, useRef, useState } from "react";
import { SyncEngine } from "./SyncEngine";

// Windows/macOS always render all three channels; Linux has an opt-in
// single-channel mode where interior/rear refs may be null or not-ready.
// Must match the IS_LINUX definition in SyncEngine.ts and VideoGrid.tsx.
const IS_LINUX =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Linux") &&
  !navigator.userAgent.includes("Android");

export function useSyncEngine(
  frontRef: RefObject<HTMLVideoElement | null>,
  interiorRef: RefObject<HTMLVideoElement | null>,
  rearRef: RefObject<HTMLVideoElement | null>,
  activeSegmentId: string | null,
): SyncEngine | null {
  const [engine, setEngine] = useState<SyncEngine | null>(null);
  const engineRef = useRef<SyncEngine | null>(null);

  useEffect(() => {
    engineRef.current?.pause();
    engineRef.current?.dispose();
    engineRef.current = null;
    setEngine(null);

    if (!activeSegmentId) return;

    const tryInit = () => {
      const f = frontRef.current;
      const i = interiorRef.current;
      const r = rearRef.current;

      if (!f || f.readyState < 2) return;
      if (engineRef.current) return;

      // On Windows/macOS, all three channels are always rendered. We must
      // wait for all three to be ready — if we initialize with a partial
      // set, the engineRef guard above prevents re-initialization, and
      // the missing slaves are permanently excluded from seek/pause/speed
      // control and drift correction (the symptom: interior/rear freeze
      // and desync after scrubbing).
      //
      // On Linux, multi-channel is opt-in (see VideoGrid.tsx IS_LINUX).
      // The single-channel diagnostic mode only renders the front panel,
      // so interior/rear refs may legitimately be null. SyncEngine's tick
      // loop and play/pause/seek/setSpeed iterate `this.slaves` — an
      // empty array is a safe no-op.
      if (!IS_LINUX) {
        if (!i || i.readyState < 2) return;
        if (!r || r.readyState < 2) return;
      }

      const slaves = [i, r].filter(
        (v): v is HTMLVideoElement => v !== null && v.readyState >= 2,
      );

      const e = new SyncEngine(f, slaves);
      e.start();
      engineRef.current = e;
      setEngine(e);
    };

    tryInit();

    const f = frontRef.current;
    const i = interiorRef.current;
    const r = rearRef.current;

    f?.addEventListener("loadeddata", tryInit);
    i?.addEventListener("loadeddata", tryInit);
    r?.addEventListener("loadeddata", tryInit);

    return () => {
      f?.removeEventListener("loadeddata", tryInit);
      i?.removeEventListener("loadeddata", tryInit);
      r?.removeEventListener("loadeddata", tryInit);
      engineRef.current?.pause();
      engineRef.current?.dispose();
      engineRef.current = null;
      setEngine(null);
    };
  }, [activeSegmentId, frontRef, interiorRef, rearRef]);

  return engine;
}
