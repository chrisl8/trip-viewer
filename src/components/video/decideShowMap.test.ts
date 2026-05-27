import { describe, expect, it } from "vitest";
import { decideShowMap } from "./decideShowMap";

describe("decideShowMap", () => {
  it("shows the map for a normal trip with GPS-capable camera", () => {
    expect(
      decideShowMap({
        gpsSupported: true,
        archiveOnly: false,
        archivedGpsPointCount: 0,
      }),
    ).toBe(true);
  });

  it("hides the map when the camera doesn't record GPS, even for a live trip", () => {
    expect(
      decideShowMap({
        gpsSupported: false,
        archiveOnly: false,
        archivedGpsPointCount: 0,
      }),
    ).toBe(false);
  });

  // Regression for the bug where archive-only trips lost their map even
  // after the `trip_gps` persistence feature shipped (commit de1acb4).
  // The previous gate was `gpsSupported && !archiveOnly`, which hid the
  // map for archive-only trips unconditionally — including ones whose
  // stitched GPS had been persisted and was already in the store.
  it("shows the map for an archive-only trip when persisted GPS exists", () => {
    expect(
      decideShowMap({
        gpsSupported: true,
        archiveOnly: true,
        archivedGpsPointCount: 1234,
      }),
    ).toBe(true);
  });

  // The other side of the same bug: archive-only trips that were
  // timelapsed BEFORE the persistence feature have no GPS source left,
  // so the map slot should still collapse rather than render an empty
  // panel forever.
  it("hides the map for an archive-only trip with no persisted GPS", () => {
    expect(
      decideShowMap({
        gpsSupported: true,
        archiveOnly: true,
        archivedGpsPointCount: 0,
      }),
    ).toBe(false);
  });

  // gpsSupported takes precedence over the archive-only fallback —
  // a Miltona single-channel trip that's been archived shouldn't show
  // a map just because the trip_gps table has zero rows for it.
  it("hides the map when GPS isn't supported, regardless of archive state", () => {
    expect(
      decideShowMap({
        gpsSupported: false,
        archiveOnly: true,
        archivedGpsPointCount: 999,
      }),
    ).toBe(false);
  });
});
