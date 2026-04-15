import { useStore } from "../state/store";

const HARD_RESYNC_S = 0.15;
const SOFT_CORRECT_S = 0.04;
const SOFT_BIAS = 0.05;

// WebKitGTK's GStreamer-backed <video> treats any `currentTime=` assignment
// as a full pipeline flush + re-decode — far heavier than Chromium's
// frame-level scrub. Running our Chromium-tuned drift correction on Linux
// causes a thrash loop: the slave never catches up inside HARD_RESYNC_S,
// so every tick re-flushes the pipeline, which starves the compositor and
// can hard-lock modest iGPUs (observed on AMD Vega 11 / VCN 1.0).
//
// On Linux we leave slaves free-running at the same playbackRate. All
// three channels come from the same firmware, same clock, same fps, so
// passive drift is in the microseconds-per-second range — imperceptible
// for dashcam playback. The drift HUD still reports live drift so we can
// confirm this empirically. Seeks and speed changes, which are one-shot
// and affect all three equally, are kept.
const IS_LINUX =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Linux") &&
  !navigator.userAgent.includes("Android");

export class SyncEngine {
  private master: HTMLVideoElement;
  private slaves: HTMLVideoElement[];
  private slaveLabels: string[];
  private disposed = false;
  private pauseIntentional = false;
  private cleanup: (() => void) | null = null;

  constructor(
    master: HTMLVideoElement,
    slaves: HTMLVideoElement[],
    slaveLabels: string[] = [],
  ) {
    this.master = master;
    this.slaves = slaves;
    // Pad/truncate labels to match slaves length so lookups are safe.
    this.slaveLabels = slaves.map((_, i) => slaveLabels[i] ?? `Slave ${i + 1}`);
    this.attachPauseGuard();
  }

  start(): void {
    const speed = useStore.getState().speed;
    this.master.playbackRate = speed;
    this.slaves.forEach((s) => (s.playbackRate = speed));

    const tick: VideoFrameRequestCallback = (_now, meta) => {
      if (this.disposed) return;

      const masterT = meta.mediaTime;
      const store = useStore.getState();
      store.setCurrentTime(masterT);
      const speed = store.speed;

      const drifts: { label: string; driftMs: number }[] = [];
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        if (slave.readyState < 2) continue;
        const drift = slave.currentTime - masterT;
        drifts.push({
          label: this.slaveLabels[i],
          driftMs: Math.round(drift * 1000),
        });

        // On Linux we deliberately do NOT correct drift — see IS_LINUX
        // comment at the top of the file. We only record the reading so
        // the drift HUD remains useful.
        if (IS_LINUX) continue;

        const absDrift = Math.abs(drift);
        if (absDrift > HARD_RESYNC_S) {
          slave.currentTime = masterT;
          slave.playbackRate = speed;
        } else if (absDrift > SOFT_CORRECT_S) {
          const bias = drift > 0 ? 1 - SOFT_BIAS : 1 + SOFT_BIAS;
          slave.playbackRate = speed * bias;
        } else if (slave.playbackRate !== speed) {
          slave.playbackRate = speed;
        }
      }

      if (store.showDriftHud) {
        store.setDrift(drifts);
      }

      this.master.requestVideoFrameCallback(tick);
    };

    this.master.requestVideoFrameCallback(tick);
  }

  dispose(): void {
    this.disposed = true;
    this.cleanup?.();
  }

  private attachPauseGuard(): void {
    const m = this.master;
    const onPause = () => {
      if (this.disposed || this.pauseIntentional) return;
      const { isPlaying } = useStore.getState();
      if (isPlaying && m.paused && !m.ended) {
        m.play().then(() => {
          this.slaves.forEach((s) => {
            if (s.paused && !s.ended) s.play().catch(() => {});
          });
        }).catch(() => {});
      }
    };
    m.addEventListener("pause", onPause);
    this.cleanup = () => m.removeEventListener("pause", onPause);
  }

  async play(): Promise<void> {
    this.pauseIntentional = false;
    try {
      const speed = useStore.getState().speed;
      this.master.playbackRate = speed;
      this.slaves.forEach((s) => (s.playbackRate = speed));
      await this.master.play();
      await Promise.all(this.slaves.map((s) => s.play()));
      useStore.getState().setIsPlaying(true);
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") return;
      console.error("SyncEngine.play failed:", e);
      useStore.getState().setError(
        e instanceof Error ? e.message : "playback failed",
      );
    }
  }

  pause(): void {
    this.pauseIntentional = true;
    this.master.pause();
    this.slaves.forEach((s) => s.pause());
    useStore.getState().setIsPlaying(false);
  }

  seek(t: number): void {
    const duration = Number.isFinite(this.master.duration)
      ? this.master.duration
      : Infinity;
    const clamped = Math.min(Math.max(0, t), duration);
    this.master.currentTime = clamped;
    this.slaves.forEach((s) => (s.currentTime = clamped));
    useStore.getState().setCurrentTime(clamped);
  }

  setSpeed(rate: number): void {
    this.master.playbackRate = rate;
    this.slaves.forEach((s) => (s.playbackRate = rate));
  }
}
