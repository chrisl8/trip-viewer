import { describe, expect, it } from "vitest";
import type { GpsPoint } from "../../types/model";
import { routeRuns } from "./routeContinuity";

const point = (tOffsetS: number, overrides: Partial<GpsPoint> = {}): GpsPoint => ({
  tOffsetS, lat: 53.5, lon: 10, speedMps: 10, headingDeg: 90,
  altitudeM: 0, fixOk: true, ...overrides,
});

describe("GPS route continuity", () => {
  it("keeps a continuous route", () => {
    expect(routeRuns([point(0), point(1), point(2)])).toHaveLength(1);
  });
  it("does not bridge missing or rejected GPS records", () => {
    expect(routeRuns([point(0), point(1), point(4), point(5)]).map(r => r.length)).toEqual([2, 2]);
    expect(routeRuns([point(0), point(1, { fixOk: false }), point(2)])).toHaveLength(2);
    expect(routeRuns([point(0), point(1, { lat: NaN }), point(2)])).toHaveLength(2);
    expect(routeRuns([point(0), point(1, { lon: 181 }), point(2)])).toHaveLength(2);
  });
  it("splits reversed and duplicate timestamps", () => {
    expect(routeRuns([point(1), point(0)])).toHaveLength(2);
    expect(routeRuns([point(1), point(1)])).toHaveLength(2);
  });
  it("does not draw a line across the map at the dateline", () => {
    expect(routeRuns([point(0, { lon: 179.9 }), point(1, { lon: -179.9 })])).toHaveLength(2);
  });
  it("accepts valid southern, western and zero coordinates", () => {
    expect(routeRuns([point(0, { lat: -33, lon: -70 }), point(1, { lat: -33, lon: -70 })])).toHaveLength(1);
    expect(routeRuns([point(0, { lat: 0, lon: 0 }), point(1, { lat: 0, lon: 0 })])).toHaveLength(1);
  });
});
