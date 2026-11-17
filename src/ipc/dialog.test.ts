import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";
import { pickFolder } from "./dialog";
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
beforeEach(() => {
  vi.mocked(open).mockReset();
  let stored: string | null = null;
  vi.stubGlobal("window", { localStorage: {
    getItem: () => stored,
    setItem: (_key: string, value: string) => { stored = value; },
  } });
});
afterEach(() => vi.unstubAllGlobals());
it("starts folder selection in the remembered archive while allowing cancellation", async () => {
  vi.mocked(open).mockResolvedValueOnce(null);
  expect(await pickFolder("Select archive", "/archive/last-selected")).toBeNull();
  expect(open).toHaveBeenCalledWith({ directory: true, multiple: false,
    title: "Select archive", defaultPath: "/archive/last-selected" });
});
it("remembers archive and import-source folders separately and retains them after cancellation", async () => {
  vi.mocked(open).mockResolvedValueOnce("/archive/chosen").mockResolvedValueOnce("/source/chosen")
    .mockResolvedValueOnce(null).mockResolvedValueOnce(null);
  await pickFolder("Archive");
  await pickFolder("Import", undefined, "import-source");
  await pickFolder("Archive");
  expect(vi.mocked(open).mock.lastCall?.[0]).toMatchObject({ defaultPath: "/archive/chosen" });
  await pickFolder("Import", undefined, "import-source");
  expect(vi.mocked(open).mock.lastCall?.[0]).toMatchObject({ defaultPath: "/source/chosen" });
});
