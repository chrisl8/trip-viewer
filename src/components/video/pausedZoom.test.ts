import { expect, it } from "vitest";
import { constrainZoom, NORMAL_VIEW, zoomAt } from "./pausedZoom";
it("keeps the pixel under the wheel anchor in place while zooming", () => {
  const view = zoomAt(NORMAL_VIEW, 2, { x: 20, y: 10 }, 200, 100);
  expect(view).toEqual({ scale: 2, x: -20, y: -10 });
  expect(20 * view.scale + view.x).toBe(20);
});
it("limits panning using the actual letterboxed picture", () => {
  expect(constrainZoom({ scale: 2, x: 500, y: -500 }, 200, 200, 200, 100)).toEqual({ scale: 2, x: 100, y: -0 });
});
it("caps zoom and returns to the centered view at normal size", () => {
  expect(constrainZoom({ scale: 99, x: 0, y: 0 }, 100, 100).scale).toBe(8);
  const view = constrainZoom({ scale: 1, x: 100, y: 100 }, 200, 100);
  expect(view.scale).toBe(1); expect(view.x).toBe(0); expect(view.y).toBe(0);
});
