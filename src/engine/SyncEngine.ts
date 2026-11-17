import { useStore, type PlaybackSlice } from "../state/store";
import { type CurveSegment, coverageAt, fileToConcat } from "../utils/speedCurve";

// How often to re-evaluate coverage gaps. Gaps are seconds-to-minutes
// long, so a coarse cadence is plenty — and crucially this runs on a
// timer, not `requestVideoFrameCallback`, because rVFC does not fire
// under the GStreamer playback pipeline on WebKitGTK (see the rVFC note
// below). A ~150 ms slip entering/leaving a gap is hidden by the black
// overlay and imperceptible given we don't drift-correct on WebKit.
const GAP_CHECK_INTERVAL_MS = 150;

const HARD_RESYNC_S = 0.15;
const SOFT_CORRECT_S = 0.04;
const SOFT_BIAS = 0.05;

// WebKit-based <video> pipelines (WebKitGTK + GStreamer on Linux,
// WKWebView + AVFoundation/Video Toolbox on macOS) implement any
// `currentTime=` assignment as a full pipeline flush + re-decode — far
// heavier than Chromium/Blink's frame-level scrub. Running our Chromium-
// tuned drift correction on these platforms causes a thrash loop: the
// slave never catches up inside HARD_RESYNC_S, so every tick re-flushes
// the pipeline, which starves the compositor (observed as a full GPU
// hang on AMD Vega 11 / VCN 1.0 under Linux, and as primary-view freezes
// with secondary-channel glitching on an M4 Mac mini).
//
// On these platforms we leave slaves free-running at the same
// playbackRate. All three channels come from the same firmware, same
// clock, same fps, so passive drift is in the microseconds-per-second
// range — imperceptible for dashcam playback. The drift HUD still
// reports live drift so we can confirm this empirically. Seeks and
// speed changes, which are one-shot and affect all three equally, are
// kept.
const SKIP_DRIFT_CORRECTION =
  typeof navigator !== "undefined" &&
  // WebKitGTK + GStreamer (Linux)
  ((navigator.userAgent.includes("Linux") &&
    !navigator.userAgent.includes("Android")) ||
    // WKWebView + AVFoundation (macOS)
    navigator.userAgent.includes("Mac OS X"));

// Stall watchdog. On the same WebKit-based pipelines that need
// SKIP_DRIFT_CORRECTION, a `play()` Promise can resolve before the
// decoder produces any frames past the first: the element reads
// `paused=false`, `readyState>=2`, and the UI shows the Pause button,
// but `currentTime` never advances. rVFC and `timeupdate` both go
// silent in that state — there is no event to react to — so we have
// to poll. Every WATCHDOG_INTERVAL_MS we sample each video's
// currentTime; if a video shows no progress for WATCHDOG_STALL_MS
// while not paused/ended, we kick it with pause()→play(), which
// forces GStreamer / AVFoundation to flush and reinit the pipeline.
// This mirrors the manual "pause then play again" workaround users
// hit on Linux.
const WATCHDOG_INTERVAL_MS = 500;
const WATCHDOG_STALL_MS = 1500;
const WATCHDOG_COOLDOWN_MS = 1500;
const ENABLE_STALL_WATCHDOG = SKIP_DRIFT_CORRECTION;

// WebKit pipelines cannot tolerate Chromium-style per-frame currentTime
// correction, but leaving slaves entirely free-running means every short stall
// or pipeline restart can leave a permanent offset. Reconcile slowly and only
// when the visible drift is large enough to justify a pipeline flush.
const REANCHOR_INTERVAL_MS = 1000;
const REANCHOR_DRIFT_S = 0.5;
const REANCHOR_COOLDOWN_MS = 2000;
const ENABLE_DRIFT_REANCHOR = SKIP_DRIFT_CORRECTION;

// WebKit/GStreamer occasionally drops seek/metadata completion events during
// rapid source changes. Transport operations must never wait forever.
const SEEK_WAIT_TIMEOUT_MS = 5000;
const METADATA_WAIT_TIMEOUT_MS = 5000;

interface WatchdogEntry {
  /** Last sampled `currentTime` for this video. */
  lastTime: number;
  /** `performance.now()` of the most recent sample where `currentTime`
   *  actually differed from the previous sample. Compared against the
   *  current tick's `now()` to compute how long the video has been
   *  stuck on the same frame. */
  lastChangedAt: number;
  /** `performance.now()` of the most recent kick. Prevents a continuous
   *  storm of kicks while the pipeline takes a moment to recover. */
  lastKickAt: number;
}

export class SyncEngine {
  private master: HTMLVideoElement;
  private slaves: HTMLVideoElement[];
  private slaveLabels: string[];
  private disposed = false;
  private pauseIntentional = false;
  /** Canonical user intent. This remains true during an internal speed-change
   *  pause so the button and watchdog do not mistake a synchronization barrier
   *  for a user-requested Pause. */
  private playIntent = false;
  /** Incremented for every transport operation so a superseded async play or
   *  speed transition can never restart playback after the user pressed Pause
   *  or selected another speed. */
  private transportGeneration = 0;
  private cleanups: Array<() => void> = [];
  private watchdogState: WeakMap<HTMLVideoElement, WatchdogEntry> =
    new WeakMap();
  private reconcileActionAt: WeakMap<HTMLVideoElement, number> = new WeakMap();
  // Coverage curves for tiered playback. `masterCurve` maps the master's
  // file-time → concat-time (the trip clock); each `slaveCurves[i]` maps
  // concat-time → that slave's coverage. Null in Original mode / for
  // full-coverage channels (no gaps to handle).
  private masterCurve: CurveSegment[] | null;
  private slaveCurves: (CurveSegment[] | null)[];
  // Per-slave: does this channel have coverage gaps (camera off for part
  // of the trip)? A gappy channel's file is gap-closed and SHORTER than
  // the master's, so it lives on a different file-time axis — the
  // `slave.currentTime = masterT` drift correction is invalid for it and
  // must be skipped; it stays synced via free-run + gap hold/resume.
  private slaveGappy: boolean[];
  // Slaves WE paused for a coverage gap — so global pause/play and gap
  // hold/resume don't fight over the same element.
  private gapPaused: Set<HTMLVideoElement> = new Set();
  // Per-slave "currently inside a coverage gap" state, so the gap loop
  // can act only on covered↔gap *transitions* (re-anchor + pause/resume)
  // and otherwise leave a covered slave free-running — no per-tick seeks,
  // which would thrash the WebKitGTK pipeline.
  private slaveInGap: boolean[];

  /** Total concat-seconds a curve covers (sum of segment spans). A
   *  full-coverage channel covers the whole trip; a gappy one covers
   *  less. */
  private static concatSpan(curve: CurveSegment[] | null): number {
    if (!curve) return 0;
    return curve.reduce((sum, s) => sum + (s.concatEnd - s.concatStart), 0);
  }

  constructor(
    master: HTMLVideoElement,
    slaves: HTMLVideoElement[],
    slaveLabels: string[] = [],
    masterCurve: CurveSegment[] | null = null,
    slaveCurves: (CurveSegment[] | null)[] = [],
  ) {
    this.master = master;
    this.slaves = slaves;
    // Pad/truncate labels to match slaves length so lookups are safe.
    this.slaveLabels = slaves.map((_, i) => slaveLabels[i] ?? `Slave ${i + 1}`);
    this.masterCurve = masterCurve;
    this.slaveCurves = slaves.map((_, i) => slaveCurves[i] ?? null);
    const masterSpan = SyncEngine.concatSpan(masterCurve);
    this.slaveGappy = this.slaves.map((_, i) => {
      const c = this.slaveCurves[i];
      if (!c || c.length === 0) return false;
      return SyncEngine.concatSpan(c) < masterSpan - 1.0;
    });
    this.slaveInGap = this.slaves.map(() => false);
    this.attachPauseGuard();
    this.attachPlaybackStateMirror();
    this.attachTimeUpdate();
    this.attachStallWatchdog();
    this.attachSlaveReconciler();
    this.attachGapCheck();
  }

  start(): void {
    const speed = useStore.getState().speed;
    this.master.playbackRate = speed;
    this.slaves.forEach((s) => (s.playbackRate = speed));
    this.applyAudioPolicy(speed);

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
        // Gappy slaves live on a different (shorter) file-time axis than
        // the master, so `currentTime = masterT` would be wrong. They're
        // kept in sync by free-run + the gap-check hold/resume instead.
        if (this.slaveGappy[i]) continue;
        const drift = slave.currentTime - masterT;
        drifts.push({
          label: this.slaveLabels[i],
          driftMs: Math.round(drift * 1000),
        });

        // On WebKit-based pipelines we deliberately do NOT correct drift
        // — see SKIP_DRIFT_CORRECTION comment at the top of the file. We
        // only record the reading so the drift HUD remains useful.
        if (SKIP_DRIFT_CORRECTION) continue;

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
    for (const fn of this.cleanups) fn();
    this.cleanups = [];
    // Drop any gap overlays this engine set so a stale black panel
    // doesn't linger after teardown / before the next engine paints.
    const store = useStore.getState();
    for (const label of this.slaveLabels) store.setChannelGapped(label, false);
    this.gapPaused.clear();
  }

  // Coverage-gap handling for tiered playback. On a timer (NOT rVFC —
  // dead under GStreamer playback on WebKitGTK), map the master's
  // file-time to concat-time, then for each slave with a curve decide
  // whether it has footage there: if not, hold it (pause + black
  // overlay); if so, resume a slave we held. No-op when there are no
  // slave curves (Original mode / all channels full-coverage).
  private attachGapCheck(): void {
    const masterCurve = this.masterCurve;
    if (!masterCurve || masterCurve.length === 0) return;
    if (!this.slaveCurves.some((c) => c && c.length > 0)) return;

    const tick = () => {
      if (this.disposed) return;
      if (this.master.readyState < 1) return;
      const concatT = fileToConcat(this.master.currentTime, masterCurve);
      const store = useStore.getState();
      const playing = store.isPlaying;
      for (let i = 0; i < this.slaves.length; i++) {
        const curve = this.slaveCurves[i];
        if (!curve || curve.length === 0) continue; // no curve → shares master axis
        const cov = coverageAt(concatT, curve);
        // Act only on transitions. Steady covered → free-run (no seek);
        // steady gap → stay held.
        if (!cov.covered && !this.slaveInGap[i]) {
          this.enterGap(i, cov.fileTime);
        } else if (cov.covered && this.slaveInGap[i]) {
          this.exitGap(i, cov.fileTime, playing);
        }
      }
    };

    const handle = setInterval(tick, GAP_CHECK_INTERVAL_MS);
    this.cleanups.push(() => clearInterval(handle));
    tick(); // paint the opening frame's coverage immediately
  }

  /** A slave entered a coverage gap. Snap it back to its last covered
   *  frame (`fileTime` is the gap's leading edge per `coverageAt`),
   *  undoing any overshoot the free-running element accumulated since
   *  the boundary, then pause + black-overlay. */
  private enterGap(i: number, fileTime: number): void {
    const slave = this.slaves[i];
    this.slaveInGap[i] = true;
    if (Number.isFinite(fileTime)) slave.currentTime = fileTime;
    this.gapPaused.add(slave);
    try {
      slave.pause();
    } catch {
      /* best-effort */
    }
    useStore.getState().setChannelGapped(this.slaveLabels[i], true);
  }

  /** A slave left a coverage gap. Re-anchor to its own file-time for
   *  this concat moment (the gap-closed file's next frame), then resume
   *  if globally playing. The one seek per boundary is masked by the
   *  black→video transition; steady covered playback never seeks. */
  private exitGap(i: number, fileTime: number, playing: boolean): void {
    const slave = this.slaves[i];
    this.slaveInGap[i] = false;
    this.gapPaused.delete(slave);
    if (Number.isFinite(fileTime)) slave.currentTime = fileTime;
    useStore.getState().setChannelGapped(this.slaveLabels[i], false);
    if (playing && slave.paused) slave.play().catch(() => {});
  }

  private slaveTargetAtMasterTime(i: number, masterT: number): number | null {
    if (!Number.isFinite(masterT)) return null;
    if (!this.masterCurve || this.masterCurve.length === 0) return masterT;

    const curve = this.slaveCurves[i];
    if (!curve || curve.length === 0) return masterT;

    const concatT = fileToConcat(masterT, this.masterCurve);
    const cov = coverageAt(concatT, curve);
    if (!cov.covered || !Number.isFinite(cov.fileTime)) return null;
    return cov.fileTime;
  }

  // Slow, coarse reconciliation for WebKit-based media pipelines. Per-frame
  // currentTime correction is deliberately disabled there because each seek
  // flushes the decoder. This timer only acts when a slave is visibly out of
  // alignment or has stopped while the master is still playing.
  private attachSlaveReconciler(): void {
    if (!ENABLE_DRIFT_REANCHOR) return;

    const handle = setInterval(() => {
      if (this.disposed) return;
      const store = useStore.getState();
      if (!this.playIntent && !store.isPlaying) return;
      if (
        this.master.readyState < 2 ||
        this.master.paused ||
        this.master.seeking
      ) {
        return;
      }

      const masterT = this.master.currentTime;
      const now = performance.now();
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        if (
          this.slaveInGap[i] ||
          slave.readyState < 2 ||
          slave.ended ||
          slave.seeking
        ) {
          continue;
        }

        const targetRaw = this.slaveTargetAtMasterTime(i, masterT);
        if (targetRaw === null) continue;
        const duration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        const target = Math.min(Math.max(0, targetRaw), duration);
        const drift = Math.abs(slave.currentTime - target);
        const needsResume = slave.paused;
        if (!needsResume && drift <= REANCHOR_DRIFT_S) continue;

        const last = this.reconcileActionAt.get(slave);
        if (last !== undefined && now - last < REANCHOR_COOLDOWN_MS) continue;
        this.reconcileActionAt.set(slave, now);

        const resume = () =>
          slave.play().catch((e) => {
            if (!(e instanceof DOMException && e.name === "AbortError")) {
              console.warn("[sync] reconciler: slave play() rejected:", e);
            }
          });

        if (drift > REANCHOR_DRIFT_S) {
          console.warn(
            `[sync] reconciler: ${this.slaveLabels[i]} drift ${Math.round(
              drift * 1000,
            )}ms — re-anchoring`,
          );
          slave.currentTime = target;
          this.watchdogState.delete(slave);
          if (needsResume) {
            void this.waitForPendingSeek(slave).then(resume);
          }
        } else if (needsResume) {
          void resume();
        }
      }
    }, REANCHOR_INTERVAL_MS);

    this.cleanups.push(() => clearInterval(handle));
  }

  private attachPauseGuard(): void {
    const m = this.master;
    const onPause = () => {
      if (this.disposed || this.pauseIntentional) return;
      if (this.playIntent && m.paused && !m.ended) {
        m.play().then(() => {
          this.slaves.forEach((s, i) => {
            // Leave gap-held slaves paused — see play().
            if (this.slaveInGap[i]) return;
            if (s.paused && !s.ended) s.play().catch(() => {});
          });
        }).catch(() => {});
      }
    };
    m.addEventListener("pause", onPause);
    this.cleanups.push(() => m.removeEventListener("pause", onPause));
  }

  // Mirror real master events only when they agree with current user intent.
  // A late "playing" event from a superseded play() must never flip the UI back
  // to Pause after the user has already pressed Pause.
  private attachPlaybackStateMirror(): void {
    const onPlaying = () => {
      if (this.disposed || !this.playIntent || this.master.paused) return;
      useStore.getState().setIsPlaying(true);
    };
    const onEnded = () => {
      if (this.disposed) return;
      this.playIntent = false;
      useStore.getState().setIsPlaying(false);
    };
    this.master.addEventListener("playing", onPlaying);
    this.master.addEventListener("ended", onEnded);
    this.cleanups.push(() => {
      this.master.removeEventListener("playing", onPlaying);
      this.master.removeEventListener("ended", onEnded);
    });
  }

  // Authoritative writer of store.currentTime. The rVFC tick in start()
  // ALSO writes the store, but on WebKitGTK (Linux) the rVFC callback
  // does not fire under the GStreamer playback pipeline — observed as a
  // permanently-zero timeline playhead and time counter during playback.
  // `timeupdate` is part of the HTML5 video spec and fires at ~4Hz on
  // every UA, which is plenty for the timeline indicator. On Chromium
  // (Windows) both fire; same-value writes don't churn React because
  // Zustand short-circuits identical state.
  private attachTimeUpdate(): void {
    const m = this.master;
    const onTimeUpdate = () => {
      if (this.disposed) return;
      useStore.getState().setCurrentTime(m.currentTime);
    };
    m.addEventListener("timeupdate", onTimeUpdate);
    m.addEventListener("seeked", onTimeUpdate);
    this.cleanups.push(() => {
      m.removeEventListener("timeupdate", onTimeUpdate);
      m.removeEventListener("seeked", onTimeUpdate);
    });
  }

  // Polls each video for a frozen `currentTime` while playback is
  // active and kicks any video that has stalled. See the file-level
  // ENABLE_STALL_WATCHDOG comment for why this exists. No-op outside
  // the WebKit-based platforms where the bug appears.
  private attachStallWatchdog(): void {
    if (!ENABLE_STALL_WATCHDOG) return;
    const handle = setInterval(() => {
      if (this.disposed) return;
      if (!useStore.getState().isPlaying) return;
      this.checkOne(this.master, "master");
      for (let i = 0; i < this.slaves.length; i++) {
        this.checkOne(this.slaves[i], this.slaveLabels[i]);
      }
    }, WATCHDOG_INTERVAL_MS);
    this.cleanups.push(() => clearInterval(handle));
  }

  private checkOne(v: HTMLVideoElement, label: string): void {
    // A paused / ended / not-yet-buffered video is not "stalled" —
    // forget any prior reading so the next live state starts fresh.
    if (v.paused || v.ended || v.readyState < 2) {
      this.watchdogState.delete(v);
      return;
    }
    const now = performance.now();
    const t = v.currentTime;
    const entry = this.watchdogState.get(v);
    if (!entry) {
      this.watchdogState.set(v, {
        lastTime: t,
        lastChangedAt: now,
        lastKickAt: 0,
      });
      return;
    }
    if (t !== entry.lastTime) {
      entry.lastTime = t;
      entry.lastChangedAt = now;
      return;
    }
    const stalledMs = now - entry.lastChangedAt;
    const sinceKickMs = now - entry.lastKickAt;
    if (stalledMs >= WATCHDOG_STALL_MS && sinceKickMs >= WATCHDOG_COOLDOWN_MS) {
      console.warn(
        `[sync] watchdog: ${label} stuck at ${t.toFixed(3)}s for ${Math.round(
          stalledMs,
        )}ms — kicking pipeline`,
      );
      entry.lastKickAt = now;
      this.kickStalled(v);
    }
  }

  private kickStalled(v: HTMLVideoElement): void {
    // pause()→seek-to-master (for slaves)→play() forces a wedged decoder to
    // recover without baking the stall offset into the rest of playback.
    const isMaster = v === this.master;
    const generation = this.transportGeneration;
    if (isMaster) this.pauseIntentional = true;

    try {
      v.pause();
    } catch {
      // ignore — pause is best-effort
    }

    if (!isMaster) {
      const i = this.slaves.indexOf(v);
      if (i >= 0 && !this.slaveInGap[i]) {
        const targetRaw = this.slaveTargetAtMasterTime(i, this.master.currentTime);
        if (targetRaw !== null && v.readyState >= 1) {
          const duration = Number.isFinite(v.duration) ? v.duration : Infinity;
          v.currentTime = Math.min(Math.max(0, targetRaw), duration);
          this.watchdogState.delete(v);
        }
      }
    }

    void this.waitForPendingSeek(v)
      .then(() => {
        if (
          this.disposed ||
          generation !== this.transportGeneration ||
          (!this.playIntent && !useStore.getState().isPlaying)
        ) {
          return;
        }
        return v.play();
      })
      .catch((e) => {
        if (!(e instanceof DOMException && e.name === "AbortError")) {
          console.warn("[sync] watchdog: kick play() rejected:", e);
        }
      })
      .finally(() => {
        if (isMaster) this.pauseIntentional = false;
      });
  }

  private waitForPendingSeek(video: HTMLVideoElement): Promise<void> {
    if (!video.seeking) return Promise.resolve();
    return new Promise((resolve) => {
      let settled = false;
      const finish = (reason?: string) => {
        if (settled) return;
        settled = true;
        globalThis.clearTimeout(timeout);
        video.removeEventListener("seeked", onSeeked);
        video.removeEventListener("error", onError);
        video.removeEventListener("emptied", onEmptied);
        if (reason) console.warn(`[sync] seek wait ended via ${reason}`);
        resolve();
      };
      const onSeeked = () => finish();
      const onError = () => finish("media error");
      const onEmptied = () => finish("media emptied");
      const timeout = globalThis.setTimeout(
        () => finish(`${SEEK_WAIT_TIMEOUT_MS}ms timeout`),
        SEEK_WAIT_TIMEOUT_MS,
      );
      video.addEventListener("seeked", onSeeked);
      video.addEventListener("error", onError);
      video.addEventListener("emptied", onEmptied);
    });
  }

  private waitForPendingSeeks(videos: HTMLVideoElement[]): Promise<void[]> {
    return Promise.all(videos.map((video) => this.waitForPendingSeek(video)));
  }

  private applyAudioPolicy(rate: PlaybackSlice["speed"]): void {
    const tiered = Boolean(this.masterCurve && this.masterCurve.length > 0);
    const masterAudible = rate === 1 && !tiered;

    // Slaves are never audio clocks. Muting them here as well as in React
    // prevents a visual-primary change or stale DOM property from creating
    // multiple slightly-offset audio streams ("hall"/echo).
    for (const slave of this.slaves) {
      slave.muted = true;
      slave.defaultMuted = true;
      slave.volume = 0;
    }

    if (masterAudible) {
      this.master.defaultMuted = false;
      this.master.muted = false;
      if (this.master.volume === 0) this.master.volume = 1;
    } else {
      this.master.muted = true;
      this.master.defaultMuted = true;
      this.master.volume = 0;
    }
  }

  private waitForReloadMetadata(video: HTMLVideoElement): Promise<void> {
    return new Promise((resolve) => {
      let settled = false;
      const finish = (reason?: string) => {
        if (settled) return;
        settled = true;
        globalThis.clearTimeout(timeout);
        video.removeEventListener("loadedmetadata", onLoaded);
        video.removeEventListener("error", onError);
        video.removeEventListener("abort", onAbort);
        if (reason) console.warn(`[sync] metadata wait ended via ${reason}`);
        resolve();
      };
      const onLoaded = () => finish();
      const onError = () => finish("media error");
      const onAbort = () => finish("abort");
      const timeout = globalThis.setTimeout(
        () => finish(`${METADATA_WAIT_TIMEOUT_MS}ms timeout`),
        METADATA_WAIT_TIMEOUT_MS,
      );
      video.addEventListener("loadedmetadata", onLoaded);
      video.addEventListener("error", onError);
      video.addEventListener("abort", onAbort);
    });
  }

  private async reloadMasterAt(
    mediaTime: number,
    rate: PlaybackSlice["speed"],
  ): Promise<void> {
    if (!Number.isFinite(mediaTime)) return;

    // Keep the audio branch silent while load() tears down and recreates
    // GStreamer's pipeline.
    this.master.muted = true;
    this.master.defaultMuted = true;
    this.master.volume = 0;

    const metadata = this.waitForReloadMetadata(this.master);
    this.master.load();
    await metadata;

    this.master.playbackRate = rate;
    const duration = Number.isFinite(this.master.duration)
      ? this.master.duration
      : Infinity;
    this.master.currentTime = Math.min(Math.max(0, mediaTime), duration);
    await this.waitForPendingSeek(this.master);
  }

  private startAll(): Promise<void> {
    // Start every live pipeline in the same turn of the event loop. We only
    // await the canonical master; a slow slave must not leave the UI stuck on
    // "Play" while playback is visibly continuing.
    const slaveStarts = this.slaves.map((s, i) => {
      if (this.slaveInGap[i]) return Promise.resolve();
      return s.play().catch((e) => {
        if (!(e instanceof DOMException && e.name === "AbortError")) {
          console.warn("[sync] slave play() rejected:", e);
        }
      });
    });
    const masterStart = this.master.play();
    void Promise.allSettled(slaveStarts);
    return masterStart;
  }

  private stopAllAfterFailure(): void {
    this.pauseIntentional = true;
    try {
      this.master.pause();
    } catch {
      /* best-effort */
    }
    for (const slave of this.slaves) {
      try {
        slave.pause();
      } catch {
        /* best-effort */
      }
    }
  }

  async play(): Promise<void> {
    const generation = ++this.transportGeneration;
    this.playIntent = true;
    this.pauseIntentional = false;
    useStore.getState().setIsPlaying(true);

    try {
      const speed = useStore.getState().speed;
      this.master.playbackRate = speed;
      this.slaves.forEach((s) => (s.playbackRate = speed));
      this.applyAudioPolicy(speed);

      // Pause→Play is a synchronization barrier for slaves only. The master is
      // the audible/canonical clock and remains the anchor.
      this.resyncToMaster(speed, false);
      await this.waitForPendingSeeks(this.slaves);
      if (generation !== this.transportGeneration || this.disposed) return;

      await this.startAll();
      if (generation !== this.transportGeneration || this.disposed) return;
      useStore.getState().setIsPlaying(true);
    } catch (e) {
      if (generation !== this.transportGeneration || this.disposed) return;
      this.playIntent = false;
      useStore.getState().setIsPlaying(false);
      this.stopAllAfterFailure();
      if (e instanceof DOMException && e.name === "AbortError") return;
      console.error("SyncEngine.play failed:", e);
      useStore.getState().setError(
        e instanceof Error ? e.message : "playback failed",
      );
    }
  }

  pause(): void {
    ++this.transportGeneration;
    this.playIntent = false;
    this.pauseIntentional = true;
    // Flip UI state first so every control immediately agrees with the user's
    // intent even if WebKit delivers media pause events asynchronously.
    useStore.getState().setIsPlaying(false);
    this.master.pause();
    this.slaves.forEach((s) => s.pause());
  }

  togglePlayback(): void {
    const mediaPlaying = !this.master.paused && !this.master.ended;
    if (this.playIntent || mediaPlaying) this.pause();
    else void this.play();
  }

  // `t` is a position on the MASTER's time axis (file-time in tiered
  // mode, segment-local in Original). Channels do NOT share one axis once
  // per-channel curves exist: a gappy rear lives on its own (shorter)
  // file-time. So we map the master position to the shared concat-time
  // (trip clock) and position every slave independently through its own
  // curve. Without this, seeking/tier-switching sent slaves to the
  // master's file-time and the rear landed somewhere unrelated.
  async seek(
    t: number,
    options: { resume?: boolean } = {},
  ): Promise<void> {
    const generation = ++this.transportGeneration;
    const state = useStore.getState();
    const mediaPlaying = !this.master.paused && !this.master.ended;
    const shouldResume =
      options.resume ?? (this.playIntent || mediaPlaying || state.isPlaying);

    this.playIntent = shouldResume;
    this.pauseIntentional = true;
    state.setIsPlaying(shouldResume);

    // Treat a seek as a transport transaction. WebKitGTK is especially prone
    // to pipeline errors when a second seek or play() lands while a previous
    // seek is still flushing.
    try {
      this.master.pause();
    } catch {
      /* best-effort */
    }
    for (const slave of this.slaves) {
      try {
        slave.pause();
      } catch {
        /* best-effort */
      }
    }

    const duration = Number.isFinite(this.master.duration)
      ? this.master.duration
      : Infinity;
    const clamped = Math.min(Math.max(0, t), duration);
    this.master.currentTime = clamped;

    if (!this.masterCurve || this.masterCurve.length === 0) {
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        const slaveDuration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        slave.currentTime = Math.min(Math.max(0, clamped), slaveDuration);
        this.slaveInGap[i] = false;
        this.gapPaused.delete(slave);
        state.setChannelGapped(this.slaveLabels[i], false);
      }
    } else {
      const concatT = fileToConcat(clamped, this.masterCurve);
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        const curve = this.slaveCurves[i];
        let covered = true;
        let target = clamped;

        if (curve && curve.length > 0) {
          const cov = coverageAt(concatT, curve);
          covered = cov.covered;
          if (Number.isFinite(cov.fileTime)) target = cov.fileTime;
        }

        const slaveDuration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        slave.currentTime = Math.min(Math.max(0, target), slaveDuration);
        this.slaveInGap[i] = !covered;
        state.setChannelGapped(this.slaveLabels[i], !covered);
        if (covered) this.gapPaused.delete(slave);
        else this.gapPaused.add(slave);
      }
    }

    state.setCurrentTime(clamped);
    this.watchdogState.delete(this.master);
    for (const slave of this.slaves) this.watchdogState.delete(slave);

    await this.waitForPendingSeeks([this.master, ...this.slaves]);
    if (generation !== this.transportGeneration || this.disposed) return;

    this.applyAudioPolicy(state.speed);

    if (!shouldResume) {
      this.playIntent = false;
      this.pauseIntentional = false;
      state.setIsPlaying(false);
      return;
    }

    this.pauseIntentional = false;
    try {
      await this.startAll();
      if (generation !== this.transportGeneration || this.disposed) return;
      useStore.getState().setIsPlaying(true);
    } catch (e) {
      if (generation !== this.transportGeneration || this.disposed) return;
      this.playIntent = false;
      useStore.getState().setIsPlaying(false);
      this.stopAllAfterFailure();
      if (e instanceof DOMException && e.name === "AbortError") return;
      console.error("SyncEngine.seek restart failed:", e);
      useStore.getState().setError(
        e instanceof Error ? e.message : "playback failed after seek",
      );
    }
  }

  /**
   * Re-anchor every slave to the master's current playback position exactly
   * once. This is intentionally event-driven: on WebKitGTK/macOS a
   * currentTime assignment flushes that video's decode pipeline, so continuous
   * drift correction is too expensive, but a one-shot re-anchor after a user
   * action is safe and deterministic.
   *
   * The master itself is never seeked here, which keeps the visible anchor
   * playing smoothly. Tiered/gappy channels are mapped through their own speed
   * curves instead of assuming a shared file-time axis.
   */
  resyncToMaster(
    rate: PlaybackSlice["speed"] = useStore.getState().speed,
    resumeSlaves = true,
  ): void {
    if (this.disposed || this.master.readyState < 1) return;

    const masterT = this.master.currentTime;
    if (!Number.isFinite(masterT)) return;

    const store = useStore.getState();
    const playing = resumeSlaves && store.isPlaying;
    this.master.playbackRate = rate;

    if (!this.masterCurve || this.masterCurve.length === 0) {
      // Original mode: all channels share the same segment-local time axis.
      for (let i = 0; i < this.slaves.length; i++) {
        const slave = this.slaves[i];
        slave.playbackRate = rate;
        if (slave.readyState < 1) continue;

        const duration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        const target = Math.min(Math.max(0, masterT), duration);
        slave.currentTime = target;
        this.watchdogState.delete(slave);

        this.slaveInGap[i] = false;
        this.gapPaused.delete(slave);
        store.setChannelGapped(this.slaveLabels[i], false);
        if (playing && slave.paused && !slave.ended) {
          slave.play().catch(() => {});
        }
      }
      store.setCurrentTime(masterT);
      return;
    }

    // Tiered mode: map the master's file-time to the shared trip clock,
    // then map that position independently onto each channel's own file.
    const concatT = fileToConcat(masterT, this.masterCurve);
    for (let i = 0; i < this.slaves.length; i++) {
      const slave = this.slaves[i];
      slave.playbackRate = rate;
      if (slave.readyState < 1) continue;

      const curve = this.slaveCurves[i];
      if (!curve || curve.length === 0) {
        const duration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        slave.currentTime = Math.min(Math.max(0, masterT), duration);
        this.watchdogState.delete(slave);
        if (playing && slave.paused && !slave.ended) {
          slave.play().catch(() => {});
        }
        continue;
      }

      const cov = coverageAt(concatT, curve);
      if (Number.isFinite(cov.fileTime)) {
        const duration = Number.isFinite(slave.duration)
          ? slave.duration
          : Infinity;
        slave.currentTime = Math.min(
          Math.max(0, cov.fileTime),
          duration,
        );
        this.watchdogState.delete(slave);
      }

      this.slaveInGap[i] = !cov.covered;
      store.setChannelGapped(this.slaveLabels[i], !cov.covered);
      if (cov.covered) {
        this.gapPaused.delete(slave);
        if (playing && slave.paused && !slave.ended) {
          slave.play().catch(() => {});
        }
      } else {
        this.gapPaused.add(slave);
        if (!slave.paused) {
          try {
            slave.pause();
          } catch {
            /* best-effort */
          }
        }
      }
    }
    store.setCurrentTime(masterT);
  }

  async setSpeed(rate: PlaybackSlice["speed"]): Promise<void> {
    const generation = ++this.transportGeneration;
    const state = useStore.getState();
    const mediaPlaying = !this.master.paused && !this.master.ended;
    const wasPlaying = this.playIntent || mediaPlaying;
    const previousRate = this.master.playbackRate;
    const anchorTime = this.master.currentTime;

    // Keep the user's transport intent stable while the pipelines are briefly
    // paused for a rate change. This makes Pause during the transition a real
    // cancellation rather than a race with a pending restart.
    this.playIntent = wasPlaying;
    state.setIsPlaying(wasPlaying);
    this.pauseIntentional = true;

    // Mute *before* touching playbackRate. WebKitGTK/GStreamer can emit a short
    // piece of stretched audio during the rate transition otherwise.
    this.master.muted = true;
    this.master.defaultMuted = true;
    this.master.volume = 0;
    for (const slave of this.slaves) {
      slave.muted = true;
      slave.defaultMuted = true;
      slave.volume = 0;
    }

    this.master.pause();
    this.slaves.forEach((s) => s.pause());
    this.slaves.forEach((s) => (s.playbackRate = rate));

    // Returning from any stretched rate to 1x gets a fresh master pipeline.
    // A same-time currentTime assignment was not sufficient on WebKitGTK:
    // the audio time-stretch branch could remain in a reverberant state.
    if (rate === 1 && previousRate !== 1) {
      await this.reloadMasterAt(anchorTime, rate);
    } else {
      this.master.playbackRate = rate;
      if (Number.isFinite(anchorTime) && this.master.readyState >= 1) {
        const duration = Number.isFinite(this.master.duration)
          ? this.master.duration
          : Infinity;
        this.master.currentTime = Math.min(Math.max(0, anchorTime), duration);
      }
      await this.waitForPendingSeek(this.master);
    }

    if (generation !== this.transportGeneration || this.disposed) return;

    this.resyncToMaster(rate, false);
    await this.waitForPendingSeeks(this.slaves);
    if (generation !== this.transportGeneration || this.disposed) return;

    // Only now expose audio again, and only for Original 1x.
    this.applyAudioPolicy(rate);

    if (!wasPlaying) {
      this.playIntent = false;
      state.setIsPlaying(false);
      return;
    }

    this.pauseIntentional = false;
    try {
      await this.startAll();
      if (generation !== this.transportGeneration || this.disposed) return;
      useStore.getState().setIsPlaying(true);
    } catch (e) {
      if (generation !== this.transportGeneration || this.disposed) return;
      this.playIntent = false;
      useStore.getState().setIsPlaying(false);
      this.stopAllAfterFailure();
      if (e instanceof DOMException && e.name === "AbortError") return;
      console.error("SyncEngine.setSpeed restart failed:", e);
      useStore.getState().setError(
        e instanceof Error ? e.message : "playback failed after speed change",
      );
    }
  }
}
