import { describe, expect, it } from "vitest";
import { bringOverlayToFront, cropGeometry, defaultOverlayPanel, moveOverlay, normalizeOverlayPanel, normalizeOverlayPanels, overlayBackground, overlayPanelFor, panOverlayCrop, resizeOverlay } from "./overlayLayout";

describe("overlay placement", () => {
  it("keeps Front as background independent of the visual primary, with graceful 1/2/4-channel fallback", () => {
    expect(overlayBackground(["Rear", "Front", "Interior"])).toBe("Front");
    expect(overlayBackground(["Interior", "Rear"])).toBe("Interior");
    expect(overlayBackground(["Rear"])).toBe("Rear");
    expect(overlayBackground([])).toBeNull();
    expect(overlayBackground(["A", "B", "C", "D"])).toBe("A");
    expect(defaultOverlayPanel("Rear").frame).toBe("mirror");
    expect(defaultOverlayPanel("Interior").frame).toBe("rounded");
  });
  it("constrains movement and resizing to the stage without changing the crop", () => {
    const panel = { ...defaultOverlayPanel("Rear"), crop: { zoom: 3, x: -.2, y: .4 } };
    const moved = moveOverlay(panel, 20, -20);
    expect(moved.x + moved.width).toBe(1);
    expect(moved.y).toBe(0);
    const resized = resizeOverlay(moved, 20, 20);
    expect(resized.x + resized.width).toBe(1);
    expect(resized.height).toBe(1);
    expect(resizeOverlay(panel, -20, -20)).toMatchObject({ width: .12, height: .1, crop: panel.crop });
    expect(resized.crop).toEqual(panel.crop);
    expect(panel.x).toBe(.28);
  });
  it("normalizes corrupt stored panels and treats arbitrary channel labels as data", () => {
    const panel = normalizeOverlayPanel({ x: 20, y: -1, width: 2, height: -1, order: Infinity,
      crop: { zoom: NaN, x: 99, y: -99 }, frame: "unknown" }, "Rear");
    expect(panel).toMatchObject({ x: 0, y: 0, width: 1, height: .1, frame: "mirror", crop: { zoom: 1, x: 1, y: -1 } });
    expect(normalizeOverlayPanels(null)).toEqual({});
    expect(normalizeOverlayPanels([])).toEqual({});
    expect(overlayPanelFor({}, "constructor")).toEqual(defaultOverlayPanel("constructor"));
    const labels = normalizeOverlayPanels(JSON.parse('{"__proto__":{"width":0.2,"x":0.1}}'));
    expect(overlayPanelFor(labels, "__proto__")).toMatchObject({ width: .2, x: .1 });
    expect(Object.getPrototypeOf(labels)).toBe(Object.prototype);
  });
  it("changes stacking without moving or recropping uncustomized panels", () => {
    const labels = ["Rear", "Interior", "Channel 4"];
    const before = labels.map((label, index) => overlayPanelFor({}, label, index));
    const after = bringOverlayToFront({}, labels, "Rear");
    labels.forEach((label, index) => {
      const { order: _, ...position } = before[index];
      const { order: __, ...newPosition } = after[label];
      expect(newPosition).toEqual(position);
    });
    expect(after.Rear.order).toBeGreaterThan(after.Interior.order);
    expect(after.Rear.order).toBeGreaterThan(after["Channel 4"].order);
  });
});

describe("persistent camera crop", () => {
  it("fills a wide mirror with undistorted video and pans inside it without exposed gaps", () => {
    const crop = { zoom: 1, x: 0, y: 0 };
    const geometry = cropGeometry(crop, 600, 160, 1920, 1080)!;
    expect(geometry.width).toBe(600);
    expect(geometry.height).toBe(337.5);
    expect(geometry.width / geometry.height).toBeCloseTo(16 / 9);
    expect(panOverlayCrop(crop, 5000, 5000, 600, 160, 1920, 1080)).toEqual({ zoom: 1, x: 0, y: 1 });
    for (const zoom of [1, 2, 8]) for (const x of [-1, 0, 1]) for (const y of [-1, 0, 1]) {
      const box = cropGeometry({ zoom, x, y }, 600, 160, 1920, 1080)!;
      expect(box.left).toBeLessThanOrEqual(0);
      expect(box.top).toBeLessThanOrEqual(0);
      expect(box.left + box.width).toBeGreaterThanOrEqual(600);
      expect(box.top + box.height).toBeGreaterThanOrEqual(160);
    }
  });
  it("retains the relative crop at different window sizes and source resolutions", () => {
    const crop = { zoom: 2.5, x: -.6, y: .8 };
    const small = cropGeometry(crop, 400, 100, 1920, 1080)!;
    const full = cropGeometry(crop, 800, 200, 3840, 2160)!;
    expect(full.left).toBeCloseTo(small.left * 2);
    expect(full.top).toBeCloseTo(small.top * 2);
    expect(full.width).toBeCloseTo(small.width * 2);
    expect(full.height).toBeCloseTo(small.height * 2);
    expect(cropGeometry(crop, 0, 100, 1920, 1080)).toBeNull();
    expect(panOverlayCrop(crop, 20, 20, 400, 100, 0, 0)).toEqual(crop);
  });
});
