import { useEffect, useState } from "react";
import clsx from "clsx";
import { addPlace, updatePlace, type Place } from "../../ipc/places";

interface Props {
  /** When provided, dialog starts in edit mode with this place's values. */
  editing?: Place;
  /** Pre-fill lat/lon (e.g. from the current segment's median GPS). */
  initialLat?: number;
  initialLon?: number;
  /** Pre-fill name (rarely useful, but e.g. defaulting "Home" on first open). */
  initialName?: string;
  onClose: () => void;
  onSaved: () => void;
}

const DEFAULT_RADIUS_M = 100;

export function PlaceDialog({
  editing,
  initialLat,
  initialLon,
  initialName,
  onClose,
  onSaved,
}: Props) {
  const [name, setName] = useState(
    editing?.name ?? initialName ?? "",
  );
  const [lat, setLat] = useState<string>(
    String(editing?.lat ?? initialLat ?? ""),
  );
  const [lon, setLon] = useState<string>(
    String(editing?.lon ?? initialLon ?? ""),
  );
  const [radius, setRadius] = useState<string>(
    String(editing?.radiusM ?? DEFAULT_RADIUS_M),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  function parseNum(s: string): number | null {
    const n = Number(s.trim());
    return Number.isFinite(n) ? n : null;
  }

  const latN = parseNum(lat);
  const lonN = parseNum(lon);
  const radiusN = parseNum(radius);

  const valid =
    name.trim().length > 0 &&
    latN !== null &&
    latN >= -90 &&
    latN <= 90 &&
    lonN !== null &&
    lonN >= -180 &&
    lonN <= 180 &&
    radiusN !== null &&
    radiusN > 0;

  async function onSave() {
    if (!valid || latN === null || lonN === null || radiusN === null) return;
    setBusy(true);
    setError(null);
    try {
      if (editing) {
        await updatePlace(editing.id, name.trim(), latN, lonN, radiusN);
      } else {
        await addPlace(name.trim(), latN, lonN, radiusN);
      }
      onSaved();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-96 rounded-md border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
      >
        <h2 className="mb-3 text-base font-semibold">
          {editing ? "Edit place" : "Add place"}
        </h2>
        <div className="flex flex-col gap-2">
          <label className="flex flex-col gap-1 text-xs text-neutral-400">
            Name
            <input
              autoFocus={!editing}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Home"
              className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-rose-500 focus:outline-none"
            />
          </label>
          <div className="grid grid-cols-2 gap-2">
            <label className="flex flex-col gap-1 text-xs text-neutral-400">
              Latitude
              <input
                value={lat}
                onChange={(e) => setLat(e.target.value)}
                placeholder="37.1234"
                inputMode="decimal"
                className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-rose-500 focus:outline-none"
              />
            </label>
            <label className="flex flex-col gap-1 text-xs text-neutral-400">
              Longitude
              <input
                value={lon}
                onChange={(e) => setLon(e.target.value)}
                placeholder="-122.1234"
                inputMode="decimal"
                className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-rose-500 focus:outline-none"
              />
            </label>
          </div>
          <label className="flex flex-col gap-1 text-xs text-neutral-400">
            Radius (m)
            <input
              value={radius}
              onChange={(e) => setRadius(e.target.value)}
              inputMode="decimal"
              className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-rose-500 focus:outline-none"
            />
          </label>
          {error && (
            <div className="rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
              {error}
            </div>
          )}
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            onClick={() => void onSave()}
            disabled={!valid || busy}
            className={clsx(
              "rounded-md px-3 py-1 text-sm",
              valid && !busy
                ? "bg-rose-700 text-white hover:bg-rose-600"
                : "cursor-not-allowed bg-neutral-800 text-neutral-500",
            )}
          >
            {editing ? "Save" : "Add"}
          </button>
        </div>
      </div>
    </div>
  );
}
