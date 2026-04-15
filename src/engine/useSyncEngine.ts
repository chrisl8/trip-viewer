import { RefObject, useEffect, useRef, useState } from "react";
import { SyncEngine } from "./SyncEngine";

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
      if (!f || f.readyState < 2) return;
      if (engineRef.current) return;

      // Tolerate missing slaves: the diagnostic single-channel mode in
      // VideoGrid only renders the front panel, so interior/rear refs may
      // be null. SyncEngine's tick loop and play/pause/seek/setSpeed all
      // iterate `this.slaves` — an empty array is a safe no-op.
      const slaves = [interiorRef.current, rearRef.current].filter(
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
