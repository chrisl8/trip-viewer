import { describe, expect, it, vi } from "vitest";
import { toggleViewerFullscreen } from "./viewerFullscreen";

function fixture() {
  const root = { requestFullscreen: vi.fn() };
  const host = {
    fullscreenElement: null as unknown as Element | null,
    exitFullscreen: vi.fn(async () => {
      host.fullscreenElement = null;
    }),
  };
  root.requestFullscreen.mockImplementation(async () => {
    host.fullscreenElement = root as unknown as Element;
  });
  const commit = vi.fn();
  const resync = vi.fn();
  const toggle = (label: string, current: string | null) =>
    toggleViewerFullscreen(
      root as unknown as HTMLElement,
      label,
      current,
      commit,
      resync,
      host,
    );
  return { root, host, commit, resync, toggle };
}

describe("shared viewer fullscreen", () => {
  it("opens the viewer root and reanchors only after browser entry completes", async () => {
    const f = fixture();
    let entered!: () => void;
    f.root.requestFullscreen.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          entered = resolve;
        }),
    );
    const pending = f.toggle("Front", null);
    expect(f.commit).toHaveBeenCalledWith("Front");
    expect(f.resync).not.toHaveBeenCalled();
    entered();
    await pending;
    expect(f.root.requestFullscreen).toHaveBeenCalledOnce();
    expect(f.resync).toHaveBeenCalledOnce();
  });

  it("returns from enlarged video without starting a nested video fullscreen", async () => {
    const f = fixture();
    await f.toggle("Front", null);
    await f.toggle("Front", "Front");
    expect(f.host.exitFullscreen).toHaveBeenCalledOnce();
    expect(f.root.requestFullscreen).toHaveBeenCalledOnce();
    expect(f.commit).toHaveBeenLastCalledWith(null);
    expect(f.resync).toHaveBeenCalledTimes(2);
  });

  it("changes the visible camera within one browser fullscreen root", async () => {
    const f = fixture();
    await f.toggle("Front", null);
    await f.toggle("Rear", "Front");
    expect(f.root.requestFullscreen).toHaveBeenCalledOnce();
    expect(f.host.exitFullscreen).not.toHaveBeenCalled();
    expect(f.commit).toHaveBeenLastCalledWith("Rear");
  });

  it("restores the logical view when the browser rejects entry", async () => {
    const f = fixture();
    f.root.requestFullscreen.mockRejectedValue(new Error("entry denied"));
    await expect(f.toggle("Front", null)).rejects.toThrow("entry denied");
    expect(f.commit).toHaveBeenLastCalledWith(null);
    expect(f.resync).not.toHaveBeenCalled();
  });

  it("restores the enlarged view when the browser rejects exit", async () => {
    const f = fixture();
    await f.toggle("Front", null);
    f.host.exitFullscreen.mockRejectedValue(new Error("exit denied"));
    await expect(f.toggle("Front", "Front")).rejects.toThrow("exit denied");
    expect(f.commit).toHaveBeenLastCalledWith("Front");
    expect(f.resync).toHaveBeenCalledOnce();
  });
});
