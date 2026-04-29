import { useMemo, useState } from "react";
import clsx from "clsx";
import { deletePlace, type Place } from "../../ipc/places";
import { useStore } from "../../state/store";
import { PlaceDialog } from "./PlaceDialog";

export function PlacesView() {
  const places = useStore((s) => s.places);
  const refreshPlaces = useStore((s) => s.refreshPlaces);
  const tripTagCounts = useStore((s) => s.tripTagCounts);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<Place | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  // Library-wide count of segments tagged for each place_<id>, summed
  // across all trips. Uses existing sidebar-count data, no new query.
  const countByPlaceId = useMemo(() => {
    const totals: Record<number, number> = {};
    for (const tripCounts of Object.values(tripTagCounts)) {
      for (const [name, count] of Object.entries(tripCounts)) {
        if (!name.startsWith("place_")) continue;
        const id = Number(name.slice("place_".length));
        if (!Number.isFinite(id)) continue;
        totals[id] = (totals[id] ?? 0) + count;
      }
    }
    return totals;
  }, [tripTagCounts]);

  async function onConfirmDelete() {
    if (confirmDeleteId === null) return;
    setBusy(true);
    try {
      await deletePlace(confirmDeleteId);
      await refreshPlaces();
      // Refresh sidebar counts too since place tags were just deleted.
      await useStore.getState().refreshTripTagCounts();
    } catch (e) {
      console.error("deletePlace failed", e);
    } finally {
      setBusy(false);
      setConfirmDeleteId(null);
    }
  }

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-neutral-950 text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-800 px-4 py-3">
        <div>
          <h1 className="text-lg font-semibold">Places</h1>
          <p className="text-xs text-neutral-500">
            {places.length === 0
              ? "No places yet. Add one to start tagging segments by GPS location."
              : `${places.length} place${places.length === 1 ? "" : "s"}. After adding or editing a place, run the Places scan to update tags.`}
          </p>
        </div>
        <button
          onClick={() => setAdding(true)}
          className="rounded-md bg-rose-700 px-3 py-1 text-sm text-white hover:bg-rose-600"
        >
          Add place
        </button>
      </header>

      <div className="flex-1 overflow-auto">
        {places.length === 0 ? (
          <div className="p-8 text-center text-sm text-neutral-500">
            You can add a place here with manual lat/lon, or from the player
            use <span className="text-rose-300">Save as place…</span> to
            pre-fill from the current segment's GPS.
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="sticky top-0 bg-neutral-900 text-xs uppercase tracking-wide text-neutral-400">
              <tr>
                <th className="px-3 py-2 text-left">Name</th>
                <th className="px-3 py-2 text-right">Latitude</th>
                <th className="px-3 py-2 text-right">Longitude</th>
                <th className="px-3 py-2 text-right">Radius (m)</th>
                <th className="px-3 py-2 text-right">Tagged</th>
                <th className="px-3 py-2 text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {places.map((place) => (
                <tr
                  key={place.id}
                  className="border-t border-neutral-900 hover:bg-neutral-900"
                >
                  <td className="px-3 py-1 text-rose-300">{place.name}</td>
                  <td className="px-3 py-1 text-right font-mono text-neutral-300">
                    {place.lat.toFixed(6)}
                  </td>
                  <td className="px-3 py-1 text-right font-mono text-neutral-300">
                    {place.lon.toFixed(6)}
                  </td>
                  <td className="px-3 py-1 text-right font-mono text-neutral-300">
                    {place.radiusM}
                  </td>
                  <td className="px-3 py-1 text-right text-neutral-400">
                    {countByPlaceId[place.id] ?? 0}
                  </td>
                  <td className="flex justify-end gap-2 px-3 py-1">
                    <button
                      onClick={() => setEditing(place)}
                      className="text-xs text-neutral-400 hover:text-sky-300"
                    >
                      Edit
                    </button>
                    <button
                      onClick={() => setConfirmDeleteId(place.id)}
                      className="text-xs text-neutral-400 hover:text-red-300"
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {adding && (
        <PlaceDialog
          onClose={() => setAdding(false)}
          onSaved={() => void refreshPlaces()}
        />
      )}
      {editing && (
        <PlaceDialog
          editing={editing}
          onClose={() => setEditing(null)}
          onSaved={() => void refreshPlaces()}
        />
      )}
      {confirmDeleteId !== null && (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
          onClick={() => setConfirmDeleteId(null)}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            className="w-96 rounded-md border border-neutral-700 bg-neutral-900 p-4"
          >
            <h2 className="text-base font-semibold">Delete place?</h2>
            <p className="mt-2 text-sm text-neutral-400">
              All segments currently tagged for this place will lose the
              tag. This cannot be undone, but you can add the place again
              and run the Places scan to restore tags.
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                onClick={() => setConfirmDeleteId(null)}
                className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
              >
                Cancel
              </button>
              <button
                onClick={() => void onConfirmDelete()}
                disabled={busy}
                className={clsx(
                  "rounded-md px-3 py-1 text-sm text-white",
                  busy
                    ? "cursor-not-allowed bg-neutral-700"
                    : "bg-red-700 hover:bg-red-600",
                )}
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
