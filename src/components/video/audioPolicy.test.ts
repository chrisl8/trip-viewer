import { describe, expect, it } from "vitest";
import { shouldMuteChannelAudio } from "./audioPolicy";

describe("dashcam audio policy", () => {
  it("keeps only the canonical master audible at Original 1x", () => {
    expect(shouldMuteChannelAudio(true, 1, "original")).toBe(false);
    expect(shouldMuteChannelAudio(false, 1, "original")).toBe(true);
  });

  it("mutes browser time-stretched audio at every non-1x speed", () => {
    expect(shouldMuteChannelAudio(true, 0.5, "original")).toBe(true);
    expect(shouldMuteChannelAudio(true, 2, "original")).toBe(true);
    expect(shouldMuteChannelAudio(true, 4, "original")).toBe(true);
  });

  it("mutes pre-rendered timelapse source audio", () => {
    expect(shouldMuteChannelAudio(true, 1, "8x")).toBe(true);
    expect(shouldMuteChannelAudio(true, 1, "16x")).toBe(true);
    expect(shouldMuteChannelAudio(true, 1, "60x")).toBe(true);
  });
});
