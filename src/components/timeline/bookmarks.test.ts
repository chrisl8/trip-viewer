import { describe, expect, it } from "vitest";
import { adjacentBookmark, bookmarkTimeLabel } from "./bookmarks";
describe("bookmark navigation", () => {
  it("moves strictly forward/backward without wrapping and rounds sub-millisecond clock noise", () => {
    const marks = [0, 1000, 2000, 180000];
    expect(adjacentBookmark(marks, 1.0004, -1)).toBe(0);
    expect(adjacentBookmark(marks, 0.9996, 1)).toBe(2);
    expect(adjacentBookmark(marks, 1.5, -1)).toBe(1);
    expect(adjacentBookmark(marks, 1.5, 1)).toBe(2);
    expect(adjacentBookmark(marks, 0, -1)).toBeNull();
    expect(adjacentBookmark(marks, 180, 1)).toBeNull();
    expect(adjacentBookmark([], 0, 1)).toBeNull();
    expect(adjacentBookmark([1000, 1050], 1, 1)).toBe(1.05);
  });
  it("formats millisecond positions, including long trips", () => {
    expect(bookmarkTimeLabel(180123)).toBe("3:00.123");
    expect(bookmarkTimeLabel(3600000)).toBe("60:00.000");
  });
});
