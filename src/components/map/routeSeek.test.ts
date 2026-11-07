import { expect, it } from "vitest";
import type { GpsPoint } from "../../types/model";
import { routeRuns, routeSeekTime } from "./routeSeek";
import { seekTripTime } from "../../utils/tripTime";
import type { Trip } from "../../types/model";
const p = (time: number, x: number, y = 0): GpsPoint => ({ tOffsetS: time, lat: y, lon: x, speedMps: 1, headingDeg: 0, altitudeM: 0, fixOk: true });
const project = (p: GpsPoint) => ({ x: p.lon * 100, y: p.lat * 100 });
it("interpolates the clicked route position rather than snapping to a sample", () => {
  expect(routeSeekTime([p(9, 0), p(11, 1)], { x: 25, y: 2 }, project, 0)).toBe(9.5);
});
it("never creates seek times inside GPS gaps or invalid records", () => {
  const points = [p(0, 0), p(1, 1), p(50, 2), { ...p(51, 3), fixOk: false }, p(52, 4)];
  expect(routeRuns(points).map(r => r.length)).toEqual([2, 1, 1]);
  expect(routeSeekTime(points, { x: 150, y: 0 }, project, 0)).toBeNull();
  expect(routeSeekTime([p(0, NaN)], { x: 0, y: 0 }, project, 0)).toBeNull();
});
it("prefers the current traversal at repeated locations and clamps route endpoints", () => {
  const points = [p(0, 0), p(1, 1), p(100, 0), p(101, 1)];
  expect(routeSeekTime(points, { x: 50, y: 0 }, project, 99)).toBe(100.5);
  expect(routeSeekTime(points, { x: -2, y: 0 }, project, 0)).toBe(0);
  expect(routeSeekTime(points, { x: 500, y: 0 }, project, 0)).toBeNull();
});
it("handles stationary/isolated fixes without dividing by zero", () => {
  expect(routeSeekTime([p(0, 1), p(1, 1)], { x: 100, y: 0 }, project, 0.8)).toBe(1);
  expect(routeSeekTime([p(4, 1)], { x: 100, y: 0 }, project, 0)).toBe(4);
});
it("hands trip time to the same segment/timelapse mapping as timeline seeks", () => {
  const time = routeSeekTime([p(9, 0), p(11, 1)], { x: 75, y: 0 }, project, 0)!;
  const trip = { segments: [{ id: "a", durationS: 10 }, { id: "b", durationS: 10 }] } as Trip;
  expect(seekTripTime(time, trip, "original", null)).toEqual({ mode: "original", activeSegmentId: "b", segmentLocalTime: 0.5 });
  expect(seekTripTime(time, trip, "8x", [{ concatStart: 0, concatEnd: 20, rate: 8 }])).toEqual(expect.objectContaining({ mode: "tiered", fileTime: 1.3125 }));
});

it("seeks archived timelapses whose original segment list is empty", () => {
  const trip = { segments: [] } as unknown as Trip;
  expect(seekTripTime(12, trip, "8x", [{ concatStart: 0, concatEnd: 24, rate: 8 }]))
    .toEqual({ mode: "tiered", fileTime: 1.5, virtualActiveSegmentId: null });
  expect(seekTripTime(12, trip, "original", null)).toBeNull();
});
