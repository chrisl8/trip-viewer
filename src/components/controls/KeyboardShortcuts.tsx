import { useEffect } from "react";
import type { SyncEngine } from "../../engine/SyncEngine";
import { useStore } from "../../state/store";
import type { PlaybackSlice } from "../../state/store";
import { concatToFile, fileToConcat } from "../../utils/speedCurve";

const SPEEDS: PlaybackSlice["speed"][] = [0.5, 1, 2, 4];

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

      if (e.code === "KeyM") {
        store.toggleMultiChannelEnabled();
        return;
      }

      if (!engine) return;

      // In tiered mode, arrows step in CONCAT-TIME (trip seconds),
      // not file-time. Without this, "jump 5 s" would move only
      // 5 / tier_rate seconds of trip content — imperceptible at 60x.
      // We convert currentTime (file-time) to concat-time, add the
      // step, and map back to file-time for the engine seek.
      const stepSec = (forward: boolean) => {
        const delta = (e.shiftKey ? 30 : 5) * (forward ? 1 : -1);
        if (store.sourceMode === "original" || !store.activeSpeedCurve) {
          return store.currentTime + delta;
        }
        const concatNow = fileToConcat(store.currentTime, store.activeSpeedCurve);
        const concatTarget = concatNow + delta;
        return concatToFile(concatTarget, store.activeSpeedCurve);
      };

      switch (e.code) {
        case "Space":
          e.preventDefault();
          if (store.isPlaying) engine.pause();
          else void engine.play();
          break;

        case "ArrowLeft":
          e.preventDefault();
          engine.seek(stepSec(false));
          break;

        case "ArrowRight":
          e.preventDefault();
          engine.seek(stepSec(true));
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
