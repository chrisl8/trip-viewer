import { describe, expect, it } from "vitest";
import { computeTripArchiveStatus } from "./tripArchiveStatus";

describe("computeTripArchiveStatus", () => {
  it("reports no archive when there are no jobs at all", () => {
    expect(computeTripArchiveStatus([], "trip-1")).toEqual({
      archiveExists: false,
      archiveBytes: null,
    });
  });

  it("reports no archive when the only rows for the trip aren't done", () => {
    const jobs = [
      { tripId: "trip-1", status: "pending", outputSizeBytes: null },
      { tripId: "trip-1", status: "failed", outputSizeBytes: 12345 },
      { tripId: "other", status: "done", outputSizeBytes: 99 },
    ];
    expect(computeTripArchiveStatus(jobs, "trip-1")).toEqual({
      archiveExists: false,
      archiveBytes: null,
    });
  });

  it("sums bytes across done rows when every size is known", () => {
    const jobs = [
      { tripId: "trip-1", status: "done", outputSizeBytes: 1000 },
      { tripId: "trip-1", status: "done", outputSizeBytes: 200 },
      { tripId: "trip-1", status: "pending", outputSizeBytes: null },
      { tripId: "other", status: "done", outputSizeBytes: 50_000 },
    ];
    expect(computeTripArchiveStatus(jobs, "trip-1")).toEqual({
      archiveExists: true,
      archiveBytes: 1200,
    });
  });

  // Regression for the merged-trip false negative. The merge concat
  // writes status='done' rows but didn't (until recently) populate
  // output_size_bytes — backfill_output_sizes fills them in later.
  // The dialog must still report "archive exists" during that gap,
  // even though it can't show a byte total.
  it("reports archive exists with null bytes when done rows have unknown size", () => {
    const jobs = [
      { tripId: "merged", status: "done", outputSizeBytes: null },
      { tripId: "merged", status: "done", outputSizeBytes: null },
      { tripId: "merged", status: "done", outputSizeBytes: null },
    ];
    expect(computeTripArchiveStatus(jobs, "merged")).toEqual({
      archiveExists: true,
      archiveBytes: null,
    });
  });

  // Mixed case: some done rows have size, others don't. We refuse to
  // show a partial total — better to omit the byte count than to
  // mislead the user with one that's lower than reality.
  it("returns null bytes when only some done rows have a known size", () => {
    const jobs = [
      { tripId: "trip-1", status: "done", outputSizeBytes: 500 },
      { tripId: "trip-1", status: "done", outputSizeBytes: null },
    ];
    expect(computeTripArchiveStatus(jobs, "trip-1")).toEqual({
      archiveExists: true,
      archiveBytes: null,
    });
  });

  it("returns no archive when the trip id is null", () => {
    const jobs = [{ tripId: "trip-1", status: "done", outputSizeBytes: 500 }];
    expect(computeTripArchiveStatus(jobs, null)).toEqual({
      archiveExists: false,
      archiveBytes: null,
    });
  });
});
