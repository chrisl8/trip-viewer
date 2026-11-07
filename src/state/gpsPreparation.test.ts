import { beforeEach, expect, it, vi } from "vitest";
import { ensureTripGps } from "../ipc/gps";
import type { GpsPoint, Trip } from "../types/model";
import { prepareTripGps } from "./gpsPreparation";
import { useStore } from "./store";
vi.mock("../ipc/gps", () => ({ ensureTripGps: vi.fn(), loadTripGps: vi.fn() }));
const trip = { id: "trip", segments: [] } as unknown as Trip;
const points = [{ tOffsetS: 1, lat: 1 }] as unknown as GpsPoint[];
beforeEach(() => {
  vi.mocked(ensureTripGps).mockReset();
  useStore.setState({ gpsPreparationByTrip: {}, gpsProgressByTrip: {}, tripGpsByTrip: {}, isPlaying: true });
});
it("does not rejoin or publish a previous archive's GPS for an identical trip ID", async () => {
  useStore.getState().setCurrentArchive({ root: "/archive/A", label: "A" });
  const old = pendingProgress();
  useStore.getState().setCurrentArchive({ root: "/archive/B", label: "B" });
  const next = pendingProgress();
  expect(old.request).not.toBe(next.request);
  old.progress({ kind: "started", attempt: 1, total: 2 });
  old.complete(points);
  await old.request;
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("loading");
  next.complete([]);
  await next.request;
  expect(useStore.getState().tripGpsByTrip.trip).toEqual([]);
});
it("shows background work without pausing transport, then publishes the cached GPS", async () => {
  let complete!: (points: GpsPoint[]) => void;
  vi.mocked(ensureTripGps).mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }));
  const request = prepareTripGps(trip);
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("loading");
  expect(useStore.getState().isPlaying).toBe(true);
  complete(points);
  await request;
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("ready");
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(points);
});
it("retains previous GPS and reports a failed refresh without disabling playback", async () => {
  useStore.setState({ tripGpsByTrip: { trip: points } });
  vi.mocked(ensureTripGps).mockRejectedValueOnce(new Error("read failed"));
  await expect(prepareTripGps(trip)).rejects.toThrow("read failed");
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("error");
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(points);
  expect(useStore.getState().isPlaying).toBe(true);
});
it("keeps the busy state and ignores an older result when another refresh is queued", async () => {
  const complete: ((points: GpsPoint[]) => void)[] = [];
  vi.mocked(ensureTripGps).mockImplementation(() => new Promise(resolve => { complete.push(resolve); }));
  const first = prepareTripGps(trip);
  const second = prepareTripGps(trip, { segment: ["Front"] }, true);
  complete[0](points);
  await first;
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("loading");
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
  complete[1]([]);
  await second;
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("ready");
  expect(useStore.getState().tripGpsByTrip.trip).toEqual([]);
});

const segmented = { id: "trip", segments: [{ durationS: 10 }, { durationS: 20 }] } as unknown as Trip;
function pendingProgress(force = false) {
  let progress!: NonNullable<Parameters<typeof ensureTripGps>[3]>;
  let complete!: (points: GpsPoint[]) => void;
  let fail!: (reason: Error) => void;
  vi.mocked(ensureTripGps).mockImplementationOnce((_trip, _excluded, _force, callback) => {
    progress = callback!;
    return new Promise((resolve, reject) => { complete = resolve; fail = reject; });
  });
  const request = prepareTripGps(segmented, {}, force);
  return { progress, complete, fail, request };
}
it("publishes each completed segment before the whole trip finishes and applies boundary corrections", async () => {
  const job = pendingProgress();
  job.progress({ kind: "started", attempt: 1, total: 2 });
  job.progress({ kind: "segment", attempt: 1, completed: 1, total: 2, updates: [{ index: 0, points }] });
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(points);
  expect(useStore.getState().gpsProgressByTrip.trip).toEqual({ completed: 1, total: 2 });
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("loading");
  const next = [{ ...points[0], tOffsetS: 11 }];
  job.progress({ kind: "segment", attempt: 1, completed: 2, total: 2, updates: [
    { index: 0, points: [] }, { index: 1, points: next },
  ] });
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(next);
  job.complete(next);
  await job.request;
});
it("retains unprocessed cached segments and rolls back previews if extraction fails", async () => {
  const cached = [points[0], { ...points[0], tOffsetS: 11 }];
  useStore.setState({ tripGpsByTrip: { trip: cached } });
  const job = pendingProgress();
  job.progress({ kind: "started", attempt: 1, total: 2 });
  job.progress({ kind: "segment", attempt: 1, completed: 1, total: 2, updates: [{ index: 0, points: [] }] });
  expect(useStore.getState().tripGpsByTrip.trip).toEqual([cached[1]]);
  job.fail(new Error("offline"));
  await expect(job.request).rejects.toThrow("offline");
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(cached);
});
it("resets a retried extraction and ignores events from the discarded attempt", async () => {
  const job = pendingProgress();
  job.progress({ kind: "started", attempt: 1, total: 2 });
  job.progress({ kind: "segment", attempt: 1, completed: 1, total: 2, updates: [{ index: 0, points }] });
  job.progress({ kind: "started", attempt: 2, total: 2 });
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
  job.progress({ kind: "segment", attempt: 1, completed: 2, total: 2, updates: [{ index: 0, points }] });
  expect(useStore.getState().gpsProgressByTrip.trip).toEqual({ completed: 0, total: 2 });
  job.complete([]);
  await job.request;
  job.progress({ kind: "segment", attempt: 2, completed: 2, total: 2, updates: [{ index: 0, points }] });
  expect(useStore.getState().tripGpsByTrip.trip).toEqual([]);
});
it("ignores superseded previews and does not retain them as a new request's baseline", async () => {
  const first = pendingProgress();
  first.progress({ kind: "started", attempt: 1, total: 2 });
  first.progress({ kind: "segment", attempt: 1, completed: 1, total: 2, updates: [{ index: 0, points }] });
  const second = pendingProgress(true);
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
  first.progress({ kind: "segment", attempt: 1, completed: 2, total: 2, updates: [{ index: 0, points }] });
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
  first.complete(points);
  await first.request;
  second.fail(new Error("offline"));
  await expect(second.request).rejects.toThrow("offline");
  expect(useStore.getState().tripGpsByTrip.trip).toBeUndefined();
});

it("rejoins the same analysis after switching A to B and back without losing progress", async () => {
  const first = pendingProgress();
  first.progress({ kind: "started", attempt: 1, total: 2 });
  first.progress({ kind: "segment", attempt: 1, completed: 1, total: 2, updates: [{ index: 0, points }] });
  useStore.getState().selectTrip("other");
  vi.mocked(ensureTripGps).mockResolvedValueOnce([]);
  await prepareTripGps({ id: "other", segments: [] } as unknown as Trip);
  useStore.getState().selectTrip("trip");
  const returning = prepareTripGps(segmented);
  expect(returning).toBe(first.request);
  expect(ensureTripGps).toHaveBeenCalledTimes(2);
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(points);
  expect(useStore.getState().gpsProgressByTrip.trip).toEqual({ completed: 1, total: 2 });
  const next = [{ ...points[0], tOffsetS: 11 }];
  first.progress({ kind: "segment", attempt: 1, completed: 2, total: 2, updates: [{ index: 1, points: next }] });
  expect(useStore.getState().tripGpsByTrip.trip).toEqual([...points, ...next]);
  first.complete([...points, ...next]);
  await returning;
  expect(useStore.getState().gpsPreparationByTrip.trip).toBe("ready");
});
it("navigation rejoins a pending forced refresh instead of discarding its exclusions", async () => {
  const job = pendingProgress(true);
  const returning = prepareTripGps(segmented);
  expect(returning).toBe(job.request);
  expect(ensureTripGps).toHaveBeenCalledTimes(1);
  job.complete(points);
  await returning;
});
it("starts a new job when the trip's segment sources changed", async () => {
  const job = pendingProgress();
  vi.mocked(ensureTripGps).mockResolvedValueOnce(points);
  await prepareTripGps({ ...segmented, segments: [] });
  job.complete([]);
  await job.request;
  expect(ensureTripGps).toHaveBeenCalledTimes(2);
  expect(useStore.getState().tripGpsByTrip.trip).toEqual(points);
});
