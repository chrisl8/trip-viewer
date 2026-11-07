export const RECOVERY_DELAYS_MS = [1000, 2000, 4000, 8000, 16000, 30000] as const;
const RECOVERY_PROBE_TIMEOUT_MS = 5000;

/**
 * Probe one failed media element at the current master time without rejoining
 * it to the SyncEngine. A successful decoded frame is stronger evidence than
 * seeked/canplay alone, both of which WebKitGTK can emit before a bad GOP fails.
 */
export function probeVideoAt(
  video: HTMLVideoElement,
  targetTime: number,
  signal?: AbortSignal,
): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    let started = false;
    let initialFrames = 0;
    let fallbackTimer: number | undefined;
    let frameCallbackId: number | null = null;

    const cleanup = () => {
      window.clearTimeout(timeout);
      window.clearTimeout(fallbackTimer);
      signal?.removeEventListener("abort", onFailure);
      video.removeEventListener("loadedmetadata", onMetadata);
      video.removeEventListener("seeked", onSeeked);
      video.removeEventListener("error", onFailure);
      if (
        frameCallbackId !== null &&
        typeof video.cancelVideoFrameCallback === "function"
      ) {
        video.cancelVideoFrameCallback(frameCallbackId);
      }
    };

    const finish = (ok: boolean) => {
      if (settled) return;
      settled = true;
      cleanup();
      try {
        video.pause();
      } catch {
        // best-effort
      }
      resolve(ok);
    };

    const onFailure = () => finish(false);

    const presentedFrames = () => {
      const quality = video.getVideoPlaybackQuality?.();
      return quality ? quality.totalVideoFrames - quality.droppedVideoFrames : 0;
    };

    const waitForFrame = () => {
      if (typeof video.requestVideoFrameCallback === "function") {
        frameCallbackId = video.requestVideoFrameCallback(() => finish(true));
      }
      // WebKit can decode and present frames without delivering rVFC for a
      // background probe. Its frame counters provide independent evidence;
      // readyState/seeked or an advancing audio clock alone do not.
      const checkFrames = () => {
        if (settled) return;
        if (!video.error && video.readyState >= 2 && presentedFrames() > initialFrames) {
          finish(true);
        } else {
          fallbackTimer = window.setTimeout(checkFrames, 50);
        }
      };
      checkFrames();
    };

    const onSeeked = () => {
      if (settled || started) return;
      started = true;
      initialFrames = presentedFrames();
      video
        .play()
        .then(() => { if (!settled) waitForFrame(); })
        .catch(() => finish(false));
    };

    const onMetadata = () => {
      const duration = Number.isFinite(video.duration)
        ? video.duration
        : Infinity;
      if (targetTime >= duration) { finish(false); return; }
      const clamped = Math.max(0, targetTime);
      try {
        video.currentTime = clamped;
        if (!video.seeking) {
          queueMicrotask(onSeeked);
        }
      } catch {
        finish(false);
      }
    };

    const timeout = window.setTimeout(
      () => finish(false),
      RECOVERY_PROBE_TIMEOUT_MS,
    );
    video.addEventListener("loadedmetadata", onMetadata, { once: true });
    video.addEventListener("seeked", onSeeked, { once: true });
    video.addEventListener("error", onFailure, { once: true });
    // load() itself emits an abort for the previous resource. Only an explicit
    // source cancellation (AbortSignal), a media error, or timeout fails this
    // new probe; the previous resource's abort is not evidence of corruption.

    signal?.addEventListener("abort", onFailure, { once: true });
    if (signal?.aborted) { finish(false); return; }
    try {
      video.load();
    } catch {
      finish(false);
    }
  });
}


/** Poll closed intervals, then apply backoff after every actual failed probe. */
export function recoveryDelayMs(attempt: number, time: number, range?: { startS?: number; endS: number | null }): number {
  if (range?.endS != null && time >= (range.startS ?? 0) && time < range.endS) return 250;
  return RECOVERY_DELAYS_MS[Math.min(attempt, RECOVERY_DELAYS_MS.length - 1)];
}
