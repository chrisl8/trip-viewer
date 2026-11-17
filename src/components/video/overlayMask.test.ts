import { expect, it } from "vitest";
import { cropGeometry } from "../../settings/overlayLayout";
import { croppedVideoMask, cropMaskInsets, overlayMaskStyle } from "./overlayMask";

it("keeps the visible video window at the stationary frame through crop and zoom", () => {
  for (const zoom of [1, 2, 8]) for (const x of [-1, -.3, 1]) for (const y of [-1, .6, 1]) {
    const rect = cropGeometry({ zoom, x, y }, 600, 160, 1920, 1080)!;
    const inset = cropMaskInsets(rect, 600, 160);
    expect(rect.left + inset.left).toBeCloseTo(0);
    expect(rect.top + inset.top).toBeCloseTo(0);
    expect(rect.width - inset.left - inset.right).toBeCloseTo(600);
    expect(rect.height - inset.top - inset.bottom).toBeCloseTo(160);
    expect(Object.values(inset).every(value => value >= 0)).toBe(true);
    // The corner curvature belongs to the 600x160 mirror, not the huge video.
    expect(croppedVideoMask(rect, 600, 160, "mirror").clipPath)
      .toContain("round 108px 108px 144px 144px / 56px 56px 72px 72px");
  }
});

it("clips both the frame surface and the composited video, including WebKit", () => {
  const surface = overlayMaskStyle("mirror");
  const video = croppedVideoMask(cropGeometry({ zoom: 2, x: 0, y: 0 }, 600, 160, 1920, 1080)!, 600, 160, "mirror");
  expect(surface.clipPath).toBe(surface.WebkitClipPath);
  expect(surface.WebkitMaskImage).toBeTruthy();
  expect(video.clipPath).toBe(video.WebkitClipPath);
  expect(video.clipPath).toMatch(/^inset\(/);
});
