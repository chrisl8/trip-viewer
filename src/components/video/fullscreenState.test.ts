import { describe, expect, it } from "vitest";
import {
  currentFullscreenView,
  exitFullscreenView,
  openFullscreenView,
  type FullscreenStack,
} from "./fullscreenState";

const dashboard = { kind: "dashboard" } as const;
const video = (label: string) => ({ kind: "video", label }) as const;

describe("fullscreen state navigation", () => {
  it("enters and exits a single video from normal view", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, video("Front"));
    expect(stack).toEqual([video("Front")]);
    stack = exitFullscreenView(stack);
    expect(stack).toEqual([]);
  });

  it("enters and exits the dashboard from normal view", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, dashboard);
    expect(stack).toEqual([dashboard]);
    stack = exitFullscreenView(stack);
    expect(stack).toEqual([]);
  });

  it("returns to single video after dashboard was opened from it", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, video("Front"));
    stack = openFullscreenView(stack, dashboard);
    expect(stack).toEqual([video("Front"), dashboard]);

    stack = exitFullscreenView(stack);
    expect(currentFullscreenView(stack)).toEqual(video("Front"));

    stack = exitFullscreenView(stack);
    expect(stack).toEqual([]);
  });

  it("returns to dashboard after a video was opened from it", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, dashboard);
    stack = openFullscreenView(stack, video("Interior"));
    expect(stack).toEqual([dashboard, video("Interior")]);

    stack = exitFullscreenView(stack);
    expect(currentFullscreenView(stack)).toEqual(dashboard);

    stack = exitFullscreenView(stack);
    expect(stack).toEqual([]);
  });

  it("preserves deeper navigation history across all directions", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, video("Front"));
    stack = openFullscreenView(stack, dashboard);
    stack = openFullscreenView(stack, video("Rear"));
    expect(stack).toEqual([video("Front"), dashboard, video("Rear")]);

    stack = exitFullscreenView(stack);
    expect(currentFullscreenView(stack)).toEqual(dashboard);
    stack = exitFullscreenView(stack);
    expect(currentFullscreenView(stack)).toEqual(video("Front"));
    stack = exitFullscreenView(stack);
    expect(stack).toEqual([]);
  });

  it("treats opening the immediately previous view as back-navigation", () => {
    let stack: FullscreenStack = [];
    stack = openFullscreenView(stack, video("Front"));
    stack = openFullscreenView(stack, dashboard);
    stack = openFullscreenView(stack, video("Front"));
    expect(stack).toEqual([video("Front")]);
  });

  it("does not duplicate the current fullscreen view", () => {
    const stack: FullscreenStack = [video("Rear")];
    expect(openFullscreenView(stack, video("Rear"))).toBe(stack);
  });
});
