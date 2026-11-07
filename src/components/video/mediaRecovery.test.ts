import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { probeVideoAt, recoveryDelayMs } from "./mediaRecovery";

class Probe extends EventTarget {
  duration = 180;
  currentTime = 0;
  seeking = true;
  readyState = 4;
  error: object | null = null;
  frame: (() => void) | null = null;
  frames = 0;
  getVideoPlaybackQuality() { return { totalVideoFrames: this.frames, droppedVideoFrames: 0 }; }
  load = vi.fn();
  pause = vi.fn();
  play = vi.fn(async () => {});
  requestVideoFrameCallback = vi.fn((callback: () => void) => { this.frame = callback; return 1; });
  cancelVideoFrameCallback = vi.fn();
  video() { return this as unknown as HTMLVideoElement; }
}

beforeEach(() => { vi.useFakeTimers(); vi.stubGlobal("window", globalThis); });
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

it("caps backoff even after a cached closed interval has ended", () => {
  expect(Array.from({ length: 8 }, (_, i) => recoveryDelayMs(i, 90))).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  expect(recoveryDelayMs(5, 89, { endS: 90 })).toBe(250);
  expect(recoveryDelayMs(5, 90, { endS: 90 })).toBe(30000);
  expect(recoveryDelayMs(1, 10, { startS: 89, endS: 100 })).toBe(2000);
});

it("requires a decoded frame, rather than seeked alone, and touches only the probe", async () => {
  const visible = new Probe();
  const probe = new Probe();
  const result = probeVideoAt(probe.video(), 91);
  probe.dispatchEvent(new Event("abort")); // normal reset of the old resource
  probe.dispatchEvent(new Event("loadedmetadata"));
  expect(probe.currentTime).toBe(91);
  probe.dispatchEvent(new Event("seeked"));
  await Promise.resolve();
  expect(probe.frame).not.toBeNull();
  probe.frame!();
  expect(await result).toBe(true);
  expect(visible.load).not.toHaveBeenCalled();
  expect(probe.cancelVideoFrameCallback).toHaveBeenCalled();
});

it("cancels old-source probes and ignores late metadata", async () => {
  const probe = new Probe();
  const abort = new AbortController();
  const result = probeVideoAt(probe.video(), 30, abort.signal);
  abort.abort();
  expect(await result).toBe(false);
  probe.dispatchEvent(new Event("loadedmetadata"));
  expect(probe.currentTime).toBe(0);
  expect(probe.play).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("accepts real frame-counter progress when WebKit omits the frame callback", async () => {
  const probe = new Probe();
  const result = probeVideoAt(probe.video(), 2);
  probe.dispatchEvent(new Event("loadedmetadata"));
  probe.dispatchEvent(new Event("seeked"));
  await vi.advanceTimersByTimeAsync(150);
  expect(probe.pause).not.toHaveBeenCalled(); // readyState alone is insufficient
  probe.frames += 1;
  await vi.advanceTimersByTimeAsync(50);
  expect(await result).toBe(true);
  expect(vi.getTimerCount()).toBe(0);
});

it("times out missing decoded frames and rejects out-of-duration probes", async () => {
  const probe = new Probe();
  const result = probeVideoAt(probe.video(), 90);
  await vi.advanceTimersByTimeAsync(5000);
  expect(await result).toBe(false);
  const pastEnd = probeVideoAt(probe.video(), 181);
  probe.dispatchEvent(new Event("loadedmetadata"));
  expect(await pastEnd).toBe(false);
  expect(probe.play).not.toHaveBeenCalled();
});
