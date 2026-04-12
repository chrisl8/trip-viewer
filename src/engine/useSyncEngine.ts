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
      const i = interiorRef.current;
      const r = rearRef.current;
      if (!f || !i || !r) return;
      if (f.readyState < 2 || i.readyState < 2 || r.readyState < 2) return;
      if (engineRef.current) return;

      const e = new SyncEngine(f, [i, r]);
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
