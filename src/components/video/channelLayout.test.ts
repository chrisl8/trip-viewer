import { describe, expect, it } from "vitest";
import { channelGridStyle } from "./channelLayout";
describe("visible camera placement", () => {
  it("fills the grid with a single visible camera", () => {
    expect(channelGridStyle(true, 0, 0, 2)).toEqual({ gridColumn: "1 / 3", gridRow: "1 / 3" });
  });
  it("gives both cameras full height when one secondary is hidden", () => {
    expect(channelGridStyle(true, 0, 1, 2).gridRow).toBe("1 / 3");
    expect(channelGridStyle(false, 0, 1, 2)).toEqual({ gridColumn: 2, gridRow: "1 / 3" });
  });
  it("stacks two secondaries beside the full-height primary", () => {
    expect(channelGridStyle(true, 0, 2, 2).gridRow).toBe("1 / 3");
    expect(channelGridStyle(false, 0, 2, 2).gridRow).toBe(1);
    expect(channelGridStyle(false, 1, 2, 2).gridRow).toBe(2);
  });
});
