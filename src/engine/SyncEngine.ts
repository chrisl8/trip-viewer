import { useStore } from "../state/store";

const HARD_RESYNC_S = 0.15;
const SOFT_CORRECT_S = 0.04;
const SOFT_BIAS = 0.05;

export class SyncEngine {
  private master: HTMLVideoElement;
  private slaves: HTMLVideoElement[];
  private disposed = false;
  private pauseIntentional = false;
  private cleanup: (() => void) | null = null;

  constructor(master: HTMLVideoElement, slaves: HTMLVideoElement[]) {
    this.master = master;
    this.slaves = slaves;
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

      const drifts = [0, 0];
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        if (slave.readyState < 2) continue;
        const drift = slave.currentTime - masterT;
        drifts[i] = drift;
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
        store.setDrift({
          interior: Math.round(drifts[0] * 1000),
          rear: Math.round(drifts[1] * 1000),
        });
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
