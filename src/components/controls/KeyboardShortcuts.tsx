import { useEffect } from "react";
import type { SyncEngine } from "../../engine/SyncEngine";
import { useStore } from "../../state/store";
import type { PlaybackSlice } from "../../state/store";

const SPEEDS: PlaybackSlice["speed"][] = [0.5, 1, 2, 4, 8];

export function KeyboardShortcuts({ engine }: { engine: SyncEngine | null }) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement
      )
        return;

      const store = useStore.getState();

      if (e.code === "KeyD") {
        store.toggleDriftHud();
        return;
      }

      if (!engine) return;

      switch (e.code) {
        case "Space":
          e.preventDefault();
          if (store.isPlaying) engine.pause();
          else void engine.play();
          break;

        case "ArrowLeft":
          e.preventDefault();
          engine.seek(store.currentTime - (e.shiftKey ? 30 : 5));
          break;

        case "ArrowRight":
          e.preventDefault();
          engine.seek(store.currentTime + (e.shiftKey ? 30 : 5));
          break;

        case "BracketLeft": {
          const idx = SPEEDS.indexOf(store.speed);
          if (idx > 0) {
            const next = SPEEDS[idx - 1];
            store.setSpeed(next);
            engine.setSpeed(next);
          }
          break;
        }

        case "BracketRight": {
          const idx = SPEEDS.indexOf(store.speed);
          if (idx < SPEEDS.length - 1) {
            const next = SPEEDS[idx + 1];
            store.setSpeed(next);
            engine.setSpeed(next);
          }
          break;
        }
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [engine]);

  return null;
}
