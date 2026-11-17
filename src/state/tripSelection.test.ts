import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useStore } from "./store";

const originalsAction = useStore.getState().deleteOriginalsForTrip;
const completeAction = useStore.getState().deleteTripCompletely;
beforeEach(() => useStore.setState({ selectedTripIds: new Set(), tripSelectionAnchorId: null }));
afterEach(() => useStore.setState({ deleteOriginalsForTrip: originalsAction, deleteTripCompletely: completeAction }));
it("adds a shift range in visible order while preserving an existing selection", () => {
  useStore.getState().toggleTripSelection("outside");
  useStore.getState().toggleTripSelection("c");
  useStore.getState().toggleTripSelection("a", { range: true, order: ["a", "b", "c"] });
  expect([...useStore.getState().selectedTripIds].sort()).toEqual(["a", "b", "c", "outside"]);
  expect(useStore.getState().tripSelectionAnchorId).toBe("a");
  useStore.getState().clearTripSelection();
  expect(useStore.getState().selectedTripIds.size).toBe(0);
  expect(useStore.getState().tripSelectionAnchorId).toBeNull();
});
it("toggles only the clicked row when the range anchor is filtered out", () => {
  useStore.getState().toggleTripSelection("hidden");
  useStore.getState().toggleTripSelection("b", { range: true, order: ["a", "b"] });
  expect([...useStore.getState().selectedTripIds].sort()).toEqual(["b", "hidden"]);
  useStore.getState().toggleTripSelection("b");
  expect([...useStore.getState().selectedTripIds]).toEqual(["hidden"]);
});
for (const mode of ["originals", "complete"] as const) {
  it(`${mode} bulk deletion stays sequential and reports partial/exception failures`, async () => {
    let finish!: () => void;
    const remove = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = () => resolve({ failures: [] }); }))
      .mockResolvedValueOnce({ failures: [{ message: "permission denied" }] })
      .mockRejectedValueOnce(new Error("offline"));
    if (mode === "originals") useStore.setState({ deleteOriginalsForTrip: remove });
    else useStore.setState({ deleteTripCompletely: remove });
    useStore.setState({ selectedTripIds: new Set(["a", "b", "c"]), tripSelectionAnchorId: "a" });
    const action = mode === "originals" ? useStore.getState().deleteOriginalsForTrips : useStore.getState().deleteTripsCompletely;
    const result = action(["a", "b", "c"]);
    expect(remove.mock.calls).toEqual([["a"]]);
    finish();
    expect(await result).toEqual({ failures: [{ tripId: "b", message: "permission denied" }, { tripId: "c", message: "offline" }] });
    expect(remove.mock.calls).toEqual([["a"], ["b"], ["c"]]);
    expect(useStore.getState().selectedTripIds.size).toBe(0);
    expect(useStore.getState().tripSelectionAnchorId).toBeNull();
  });
}
