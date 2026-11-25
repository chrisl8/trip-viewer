import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { currentArchive, openArchive } from "../ipc/archive";
import { scanFolder } from "../ipc/scanner";
import * as tripsIpc from "../ipc/trips";
import type { ScanResult, Trip } from "../types/model";
import { reloadArchive, restoreArchive, selectArchive } from "./archiveNavigation";
import { useStore } from "./store";

vi.mock("../ipc/archive", () => ({ currentArchive: vi.fn(), openArchive: vi.fn() }));
vi.mock("../ipc/scanner", async (original) => ({
  ...await original<typeof import("../ipc/scanner")>(), scanFolder: vi.fn(),
}));
const a = { root: "/archive/A", label: "A" };
const b = { root: "/archive/B", label: "B" };
const result = (id: string): ScanResult => ({ trips: [{ id, segments: [] } as unknown as Trip], errors: [] });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.mocked(openArchive).mockReset();
  vi.mocked(currentArchive).mockReset();
  vi.mocked(scanFolder).mockReset();
  useStore.setState(useStore.getInitialState(), true);
  useStore.setState({
    mergeArchiveOnlyTrips: vi.fn().mockResolvedValue(undefined),
    refreshTripTagCounts: vi.fn().mockResolvedValue(undefined),
    refreshLibrarySummary: vi.fn().mockResolvedValue(undefined),
  });
});
afterEach(() => vi.restoreAllMocks());

it("clears the old library/player and scans the canonical newly selected directory once", async () => {
  useStore.getState().setCurrentArchive(a);
  useStore.setState({ trips: result("old").trips, loadedTripId: "old", selectedTripId: "old",
    activeSegmentId: "old-segment", isPlaying: true, tripGpsByTrip: { old: [] },
    libraryFirstLoadDone: true, status: "ready" });
  const opened = deferred<typeof b>();
  vi.mocked(openArchive).mockReturnValueOnce(opened.promise);
  const choice = selectArchive("/picked/B");
  expect(useStore.getState().status).toBe("loading");
  opened.resolve(b);
  await choice;
  expect(useStore.getState()).toMatchObject({ currentArchive: b, trips: [], loadedTripId: null,
    activeSegmentId: null, isPlaying: false, tripGpsByTrip: {}, libraryFirstLoadDone: false });
  const scanned = deferred<ScanResult>();
  vi.mocked(scanFolder).mockReturnValueOnce(scanned.promise);
  const first = reloadArchive();
  expect(reloadArchive()).toBe(first);
  expect(useStore.getState().status).toBe("loading");
  expect(scanFolder).toHaveBeenCalledExactlyOnceWith(b.root);
  scanned.resolve(result("new"));
  expect(await first).toBe(true);
  expect(useStore.getState().trips[0].id).toBe("new");
  expect(useStore.getState().status).toBe("ready");
});

for (const failure of [false, true]) {
  it(`ignores an old directory's late ${failure ? "error" : "result"} after switching`, async () => {
    useStore.getState().setCurrentArchive(a);
    const old = deferred<ScanResult>();
    vi.mocked(scanFolder).mockReturnValueOnce(old.promise).mockResolvedValueOnce(result("B"));
    const first = reloadArchive();
    vi.mocked(openArchive).mockResolvedValueOnce(b);
    await selectArchive(b.root);
    await reloadArchive();
    if (failure) old.reject(new Error("old drive offline"));
    else old.resolve(result("A"));
    expect(await first).toBe(false);
    expect(useStore.getState()).toMatchObject({ currentArchive: b, status: "ready", error: null });
    expect(useStore.getState().trips[0].id).toBe("B");
  });
}

it("refreshes even when the same directory is selected again", async () => {
  vi.mocked(openArchive).mockResolvedValue(b);
  vi.mocked(scanFolder).mockResolvedValueOnce(result("before")).mockResolvedValueOnce(result("added-video"));
  await selectArchive(b.root);
  await reloadArchive();
  await selectArchive(b.root);
  await reloadArchive();
  expect(scanFolder).toHaveBeenCalledTimes(2);
  expect(useStore.getState().trips[0].id).toBe("added-video");
});

it("rescans after an in-flight scan when an import has added files", async () => {
  useStore.getState().setCurrentArchive(b);
  const beforeImport = deferred<ScanResult>();
  vi.mocked(scanFolder).mockReturnValueOnce(beforeImport.promise).mockResolvedValueOnce(result("imported"));
  const initial = reloadArchive();
  const afterImport = reloadArchive({ fresh: true });
  beforeImport.resolve(result("before-import"));
  await initial;
  expect(await afterImport).toBe(true);
  expect(scanFolder).toHaveBeenCalledTimes(2);
  expect(useStore.getState().trips[0].id).toBe("imported");
});

it("restores the backend's remembered directory and reloads its files on startup", async () => {
  vi.mocked(currentArchive).mockResolvedValueOnce(b);
  vi.mocked(scanFolder).mockResolvedValueOnce(result("restored"));
  await restoreArchive();
  await reloadArchive();
  expect(useStore.getState().currentArchive).toEqual(b);
  expect(scanFolder).toHaveBeenCalledExactlyOnceWith(b.root);
});

it("does not let a delayed startup response overwrite a user's new selection", async () => {
  const startup = deferred<typeof a>();
  vi.mocked(currentArchive).mockReturnValueOnce(startup.promise);
  const restore = restoreArchive();
  vi.mocked(openArchive).mockResolvedValueOnce(b);
  await selectArchive(b.root);
  startup.resolve(a);
  await restore;
  expect(useStore.getState().currentArchive).toEqual(b);
});

it("serializes native opens so the newest choice is also the last persisted one", async () => {
  const old = deferred<typeof a>();
  vi.mocked(openArchive).mockReturnValueOnce(old.promise).mockResolvedValueOnce(b);
  const first = selectArchive(a.root);
  await Promise.resolve();
  const second = selectArchive(b.root);
  expect(openArchive).toHaveBeenCalledTimes(1);
  old.resolve(a);
  expect(await first).toBeNull();
  expect(await second).toEqual(b);
  expect(vi.mocked(openArchive).mock.calls).toEqual([[a.root], [b.root]]);
  expect(useStore.getState().currentArchive).toEqual(b);
});

it("reports a failed open without discarding the active directory", async () => {
  useStore.getState().setCurrentArchive(a);
  vi.mocked(openArchive).mockRejectedValueOnce(new Error("cannot save settings"));
  await expect(selectArchive(b.root)).rejects.toThrow("cannot save settings");
  expect(useStore.getState()).toMatchObject({ currentArchive: a, status: "error", error: "cannot save settings" });
});

it("ignores a previous archive's delayed archive-only trip list", async () => {
  const old = deferred<Trip[]>();
  vi.spyOn(tripsIpc, "listArchiveOnlyTrips").mockReturnValueOnce(old.promise);
  useStore.getState().setCurrentArchive(a);
  const pending = useStore.getInitialState().mergeArchiveOnlyTrips();
  useStore.getState().setCurrentArchive(b);
  useStore.setState({ trips: result("B").trips });
  old.resolve([{ ...result("old-archive-only").trips[0], archiveOnly: true }]);
  await pending;
  expect(useStore.getState().trips.map(t => t.id)).toEqual(["B"]);
});

it("does not dismiss the new folder's loading placeholder when old post-scan work finishes", async () => {
  const old = deferred<void>();
  useStore.setState({ mergeArchiveOnlyTrips: vi.fn().mockReturnValueOnce(old.promise) });
  useStore.getState().setCurrentArchive(a);
  useStore.getState().setScanResult(result("A"));
  useStore.getState().setCurrentArchive(b);
  old.resolve();
  await old.promise;
  expect(useStore.getState().libraryFirstLoadDone).toBe(false);
  expect(useStore.getState().trips).toEqual([]);
});
