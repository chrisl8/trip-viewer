import type { TimelapseEncodingProgress } from "../../ipc/timelapse";

export function estimateEncodingStep(
  progress: TimelapseEncodingProgress & { receivedAtMs: number }, now: number,
) {
  const { processedSeconds, totalSeconds, elapsedSeconds, receivedAtMs } = progress;
  if (![processedSeconds, totalSeconds, elapsedSeconds, receivedAtMs].every(Number.isFinite) ||
      totalSeconds <= 0 || elapsedSeconds < 0) return null;
  const processed = Math.max(0, Math.min(processedSeconds, totalSeconds));
  const stale = now - receivedAtMs > 15_000;
  const remainingSeconds = !stale && processed > 0.001 && elapsedSeconds >= 3 && processed < totalSeconds
    ? elapsedSeconds * (totalSeconds - processed) / processed : null;
  return { percent: 100 * processed / totalSeconds, processed, total: totalSeconds, stale, remainingSeconds };
}
