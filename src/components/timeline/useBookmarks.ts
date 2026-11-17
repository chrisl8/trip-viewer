import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";

export function useBookmarks(archiveRoot: string | undefined, tripId: string | undefined) {
  const key = JSON.stringify([archiveRoot, tripId]);
  const currentKey = useRef(key);
  currentKey.current = key;
  const writes = useRef<Promise<void>>(Promise.resolve());
  const generation = useRef(0);
  const [state, setState] = useState({ key: "", times: [] as number[], ready: false, error: "" });
  useEffect(() => {
    let active = true;
    generation.current += 1;
    setState({ key, times: [], ready: false, error: "" });
    if (archiveRoot && tripId) {
      writes.current.then(() => invoke<number[]>("list_bookmarks", { archiveRoot, tripId })).then(times => {
        if (active) setState({ key, times, ready: true, error: "" });
      }).catch(error => {
        if (active) setState({ key, times: [], ready: false, error: `Could not load bookmarks: ${String(error)}` });
      });
    }
    return () => { active = false; };
  }, [key, archiveRoot, tripId]);
  const visible = state.key === key ? state : { times: [], ready: false, error: "" };
  const setBookmark = async (timeMs: number, present: boolean) => {
    if (!visible.ready || !archiveRoot || !tripId) return;
    const attempt = generation.current;
    try {
      const operation = writes.current.then(() => invoke<void>("set_bookmark", { archiveRoot, tripId, timeMs, present }));
      writes.current = operation.catch(() => {});
      await operation;
      if (currentKey.current === key && generation.current === attempt) setState(s => ({ ...s, error: "", times: (present
        ? [...s.times.filter(t => t !== timeMs), timeMs]
        : s.times.filter(t => t !== timeMs)).sort((a, b) => a - b) }));
    } catch (error) {
      if (currentKey.current === key && generation.current === attempt) setState(s => ({ ...s, error: `Could not save bookmark: ${String(error)}` }));
    }
  };
  return { ...visible, setBookmark };
}
