import { beforeEach, expect, it, vi } from "vitest";
import { Channel, invoke } from "@tauri-apps/api/core";
import { ensureTripGps } from "./gps";
import type { Trip } from "../types/model";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage = () => {}; } }));
const trip = {
  id: "trip",
  segments: [
    { id: "archived", durationS: 60, isTombstone: true, gpsSupported: true, channels: [] },
    { id: "no-gps", durationS: 30, gpsSupported: false, channels: [{ label: "Front", filePath: "no-gps" }] },
    { id: "current", durationS: 180, gpsSupported: true, channels: [
      { label: "Front", filePath: "front" }, { label: "Rear", filePath: "rear" },
    ] },
  ],
} as unknown as Trip;
beforeEach(() => vi.mocked(invoke).mockReset());
it("keeps archived and GPS-less durations while excluding the failed camera", async () => {
  vi.mocked(invoke).mockResolvedValue([]);
  await ensureTripGps(trip, { current: ["Front"] }, true);
  expect(invoke).toHaveBeenCalledWith("ensure_trip_gps", expect.objectContaining({
    force: true,
    segments: [
      expect.objectContaining({ segmentId: "archived", durationS: 60, channels: [] }),
      expect.objectContaining({ segmentId: "no-gps", durationS: 30, channels: [] }),
      expect.objectContaining({ segmentId: "current", channels: [{ label: "Rear", path: "rear" }] }),
    ],
  }));
});
it("serializes concurrent failures and continues after a failed refresh", async () => {
  let reject!: (reason: Error) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; })).mockResolvedValueOnce([]);
  const first = ensureTripGps(trip, { current: ["Front"] }, true);
  const rejected = expect(first).rejects.toThrow("offline");
  const second = ensureTripGps(trip, { current: ["Front", "Rear"] }, true);
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  reject(new Error("offline"));
  await rejected;
  await second;
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(invoke).toHaveBeenLastCalledWith("ensure_trip_gps", expect.objectContaining({
    segments: expect.arrayContaining([expect.objectContaining({ segmentId: "current", channels: [] })]),
  }));
});

it("attaches a per-request progress channel when requested", async () => {
  const progress = vi.fn();
  vi.mocked(invoke).mockImplementationOnce(async (_command, args) => {
    const channel = (args as { onProgress: Channel<unknown> }).onProgress;
    channel.onmessage({ kind: "started", attempt: 1, total: 3 });
    return [];
  });
  await ensureTripGps(trip, {}, false, progress);
  expect(progress).toHaveBeenCalledWith({ kind: "started", attempt: 1, total: 3 });
});
