import { afterEach, expect, it, vi } from "vitest";
import { getLayoutPreferences, saveLayoutPreferences, subscribeLayoutPreferences } from "./layout";
afterEach(() => vi.unstubAllGlobals());
it("persists camera choices while retaining existing layout preferences", () => {
  let stored = JSON.stringify({ primaryShare: .7, dashboardTimelineCollapsed: true });
  vi.stubGlobal("window", { localStorage: { getItem: () => stored, setItem: (_: string, value: string) => { stored = value; } } });
  expect(getLayoutPreferences().hiddenChannels).toEqual([]);
  saveLayoutPreferences({ hiddenChannels: ["Front", "Rear", "Rear"] });
  const restored = getLayoutPreferences();
  expect(restored.hiddenChannels).toEqual(["Front", "Rear"]);
  expect(restored.primaryShare).toBe(.7);
  expect(restored.dashboardTimelineCollapsed).toBe(true);
  expect(restored.cameraLayout).toBe("grid");
  expect(restored.overlayPanels).toEqual({});
});
it("retains classic sizes and overlay crop when switching modes and restarting", () => {
  let stored = JSON.stringify({ primaryShare: .72, secondarySplit: .35, mapShare: .3, timelineHeightPx: 120 });
  vi.stubGlobal("window", { localStorage: { getItem: () => stored, setItem: (_: string, value: string) => { stored = value; } } });
  saveLayoutPreferences({ cameraLayout: "overlay", overlayPanels: {
    Rear: { x: .2, y: .1, width: .5, height: .25, order: 2, frame: "mirror", crop: { zoom: 2, x: -.4, y: .7 } },
  } });
  saveLayoutPreferences({ cameraLayout: "grid" });
  saveLayoutPreferences({ hiddenChannels: ["Interior"] });
  saveLayoutPreferences({ cameraLayout: "overlay" });
  expect(getLayoutPreferences()).toMatchObject({ cameraLayout: "overlay", primaryShare: .72, secondarySplit: .35,
    mapShare: .3, timelineHeightPx: 120, hiddenChannels: ["Interior"],
    overlayPanels: { Rear: { x: .2, y: .1, width: .5, crop: { zoom: 2, x: -.4, y: .7 } } } });
});
it("notifies a mounted player of saved settings and retains them during resizing", () => {
  let stored = JSON.stringify({ hiddenChannels: ["Interior"], primaryShare: .7 });
  vi.stubGlobal("window", { localStorage: { getItem: () => stored, setItem: (_: string, value: string) => { stored = value; } } });
  const seen: string[][] = [];
  const unsubscribe = subscribeLayoutPreferences(() => {
    seen.push(getLayoutPreferences().hiddenChannels);
  });
  try {
    saveLayoutPreferences({ hiddenChannels: ["Interior", "Rear"] });
    expect(seen).toEqual([["Interior", "Rear"]]);
    saveLayoutPreferences({ primaryShare: .8 });
    expect(getLayoutPreferences().hiddenChannels).toEqual(["Interior", "Rear"]);
    expect(getLayoutPreferences().primaryShare).toBe(.8);
  } finally {
    unsubscribe();
  }
  const count = seen.length;
  saveLayoutPreferences({ hiddenChannels: [] });
  expect(seen).toHaveLength(count);
});
