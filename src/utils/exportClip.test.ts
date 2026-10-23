import { describe, expect, it } from "vitest";
import type { Segment, Trip } from "../types/model";
import {
  audioArgsFromPercent,
  compareLabels,
  defaultClipFileName,
  formatClock,
  parseClockInput,
  tripChannelLabels,
  tripSegmentsForExport,
  validateRange,
} from "./exportClip";

function ch(label: string, filePath: string) {
  return {
    label,
    filePath,
    width: null,
    height: null,
    fpsNum: null,
    fpsDen: null,
    codec: null,
    hasGpmdTrack: false,
  };
}

function seg(id: string, durationS: number, channels: ReturnType<typeof ch>[]): Segment {
  return {
    id,
    startTime: "2026-10-23T14:21:00",
    durationS,
    isEvent: false,
    channels,
    cameraKind: "wolfBox",
    gpsSupported: true,
    sizeBytes: null,
  };
}

const trip: Trip = {
  id: "t1",
  startTime: "2026-10-23T14:21:00",
  endTime: "2026-10-23T14:23:00",
  segments: [
    seg("a", 60, [ch("Rear", "/r/a.mp4"), ch("Front", "/f/a.mp4")]),
    seg("b", 60, [ch("Front", "/f/b.mp4"), ch("Interior", "/c/b.mp4")]),
  ],
  cameraKind: "wolfBox",
  gpsSupported: true,
};

describe("parseClockInput", () => {
  it("accepts seconds and clock forms", () => {
    expect(parseClockInput("90")).toBe(90);
    expect(parseClockInput("12.5")).toBe(12.5);
    expect(parseClockInput("1:30")).toBe(90);
    expect(parseClockInput("0:07.5")).toBe(7.5);
    expect(parseClockInput("1:02:03")).toBe(3723);
    expect(parseClockInput("  2:00 ")).toBe(120);
  });
  it("rejects anything else", () => {
    for (const bad of ["", "abc", "1:2:3:4", "-5", "1:30.5:00", "1,5", ":30", "1:"]) {
      expect(parseClockInput(bad), bad).toBeNull();
    }
  });
});

describe("formatClock", () => {
  it("formats minutes and hours", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(7.9)).toBe("0:07");
    expect(formatClock(90)).toBe("1:30");
    expect(formatClock(3723)).toBe("1:02:03");
    expect(formatClock(-4)).toBe("0:00");
  });
});

describe("channel labels", () => {
  it("sorts canonical labels first, then alphabetically", () => {
    expect(["Rear", "Zed", "Front", "Alpha", "Interior"].sort(compareLabels)).toEqual([
      "Front",
      "Interior",
      "Rear",
      "Alpha",
      "Zed",
    ]);
  });
  it("collects labels across all segments", () => {
    expect(tripChannelLabels(trip)).toEqual(["Front", "Interior", "Rear"]);
  });
});

describe("tripSegmentsForExport", () => {
  it("maps segments and their channel files", () => {
    const segs = tripSegmentsForExport(trip);
    expect(segs).toHaveLength(2);
    expect(segs[0].durationS).toBe(60);
    expect(segs[0].files).toEqual([
      { label: "Rear", path: "/r/a.mp4" },
      { label: "Front", path: "/f/a.mp4" },
    ]);
  });
});

describe("defaultClipFileName", () => {
  it("uses the trip's wall-clock time plus the offset", () => {
    expect(defaultClipFileName(trip, 0)).toBe("TripViewer_20261023-142100.mp4");
    expect(defaultClipFileName(trip, 52)).toBe("TripViewer_20261023-142152.mp4");
    expect(defaultClipFileName(trip, 3600)).toBe("TripViewer_20261023-152100.mp4");
  });
  it("rolls over midnight", () => {
    const late: Trip = { ...trip, startTime: "2026-12-31T23:59:30" };
    expect(defaultClipFileName(late, 45)).toBe("TripViewer_20270101-000015.mp4");
  });
  it("falls back when the timestamp is unreadable", () => {
    expect(defaultClipFileName({ ...trip, startTime: "??" }, 5)).toBe("TripViewer_clip.mp4");
  });
});

describe("validateRange", () => {
  it("clamps the length to the end of the trip", () => {
    expect(validateRange(100, 60, 120)).toEqual({ startS: 100, durationS: 20 });
    expect(validateRange(0, 30, 120)).toEqual({ startS: 0, durationS: 30 });
  });
  it("rejects bad input", () => {
    expect(validateRange(null, 30, 120)).toBeNull();
    expect(validateRange(0, null, 120)).toBeNull();
    expect(validateRange(120, 30, 120)).toBeNull();
    expect(validateRange(-1, 30, 120)).toBeNull();
    expect(validateRange(0, 0.5, 120)).toBeNull();
  });
});

describe("audioArgsFromPercent", () => {
  it("keeps channels above 0% in canonical order, as multipliers", () => {
    expect(audioArgsFromPercent({ Rear: 50, Front: 100, Interior: 0 })).toEqual([
      { label: "Front", volume: 1 },
      { label: "Rear", volume: 0.5 },
    ]);
  });
  it("is silent when everything is 0%", () => {
    expect(audioArgsFromPercent({ Front: 0 })).toEqual([]);
  });
});
