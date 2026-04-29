import { useEffect, useState } from "react";
import clsx from "clsx";
import { useStore } from "../../state/store";
import {
  type TimelapseMergeAssessment,
  type TimelapseMergeStrategy,
} from "../../ipc/trips";
import type { Trip } from "../../types/model";
import { formatTripStart } from "../../utils/format";

interface Props {
  marked: Trip[];
  onClose: () => void;
}

/**
 * Confirmation dialog for the trip-merge action.
 *
 * Two paths:
 *  - No marked trip has any timelapse outputs → show a simple
 *    "Merge N trips?" confirm with no strategy picker.
 *  - At least one marked trip has outputs → call `assess_trip_merge`
 *    to get a per-(tier, channel) feasibility matrix and let the user
 *    pick "Concatenate where possible" or "Discard all".
 */
export function MergeTripsDialog({ marked, onClose }: Props) {
  const mergeMarkedTrips = useStore((s) => s.mergeMarkedTrips);
  const assessMergeMarked = useStore((s) => s.assessMergeMarked);

  const [assessment, setAssessment] =
    useState<TimelapseMergeAssessment | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [strategy, setStrategy] =
    useState<TimelapseMergeStrategy>("concatWherePossible");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void assessMergeMarked()
      .then((a) => {
        if (cancelled) return;
        setAssessment(a);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setErrorMessage(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [assessMergeMarked]);

  // Sorted earliest-first so the user sees which trip becomes the
  // primary (it's the first row in this list). The store action uses
  // the same earliest-start rule.
  const orderedMarked = [...marked].sort((a, b) =>
    a.startTime.localeCompare(b.startTime),
  );

  async function onConfirm() {
    setBusy(true);
    setErrorMessage(null);
    try {
      // When no timelapses exist the strategy is irrelevant — pass
      // discardAll because it's a no-op in that case.
      const effective: TimelapseMergeStrategy = assessment?.hasAnyTimelapses
        ? strategy
        : "discardAll";
      await mergeMarkedTrips(effective);
      onClose();
    } catch (e) {
      setErrorMessage(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }

  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/60">
      <div className="w-[28rem] rounded-md border border-neutral-700 bg-neutral-900 p-4 shadow-lg">
        <h2 className="text-base font-semibold">
          Merge {marked.length} trips?
        </h2>
        <p className="mt-2 text-sm text-neutral-400">
          The earliest-starting trip becomes the primary; the others fold
          into it. The merge is recorded persistently so a folder rescan
          won&apos;t split them apart.
        </p>

        <div className="mt-3 max-h-32 space-y-1 overflow-y-auto rounded border border-neutral-800 bg-neutral-950 p-2 text-xs">
          {orderedMarked.map((t, idx) => (
            <div key={t.id} className="flex items-center justify-between">
              <span className="text-neutral-200">
                {formatTripStart(t.startTime)}
              </span>
              <span className="text-neutral-500">
                {idx === 0 ? "primary" : "absorbed"}
              </span>
            </div>
          ))}
        </div>

        {loading && (
          <p className="mt-3 text-xs text-neutral-500">
            Checking timelapse outputs…
          </p>
        )}

        {!loading && assessment && assessment.hasAnyTimelapses && (
          <div className="mt-3 space-y-2">
            <p className="text-sm text-neutral-300">
              Existing timelapse outputs:
            </p>
            <table className="w-full border border-neutral-800 text-xs">
              <thead className="bg-neutral-950 text-neutral-500">
                <tr>
                  <th className="px-2 py-1 text-left">Tier</th>
                  <th className="px-2 py-1 text-left">Channel</th>
                  <th className="px-2 py-1 text-left">State</th>
                </tr>
              </thead>
              <tbody>
                {assessment.tuples.map((t) => (
                  <tr
                    key={`${t.tier}-${t.channel}`}
                    className="border-t border-neutral-800"
                  >
                    <td className="px-2 py-1 text-neutral-300">{t.tier}</td>
                    <td className="px-2 py-1 text-neutral-300">{t.channel}</td>
                    <td className="px-2 py-1">
                      {t.status === "concatenable" ? (
                        <span className="text-emerald-300">
                          ✓ concat-ready (every source has it)
                        </span>
                      ) : (
                        <span className="text-amber-300">
                          ⚠ partial (only{" "}
                          {1 +
                            t.absorbedWithOutput.length}{" "}
                          of {marked.length} sources have it)
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>

            <fieldset className="mt-2 space-y-1.5">
              <legend className="text-xs font-medium text-neutral-300">
                Strategy
              </legend>
              <label className="flex cursor-pointer items-start gap-2 text-xs text-neutral-300">
                <input
                  type="radio"
                  name="merge-strategy"
                  checked={strategy === "concatWherePossible"}
                  onChange={() => setStrategy("concatWherePossible")}
                  disabled={busy}
                  className="mt-0.5"
                />
                <span>
                  <span className="font-medium">Concatenate where possible</span>
                  <br />
                  <span className="text-neutral-500">
                    Splice concat-ready tuples losslessly into the merged
                    trip; partial tuples are dropped — click Rebuild on the
                    merged trip later to fill them in.
                  </span>
                </span>
              </label>
              <label className="flex cursor-pointer items-start gap-2 text-xs text-neutral-300">
                <input
                  type="radio"
                  name="merge-strategy"
                  checked={strategy === "discardAll"}
                  onChange={() => setStrategy("discardAll")}
                  disabled={busy}
                  className="mt-0.5"
                />
                <span>
                  <span className="font-medium">Discard all</span>
                  <br />
                  <span className="text-neutral-500">
                    Drop every existing timelapse_jobs row. The merged trip
                    starts with no encoded outputs; you click Rebuild to
                    re-encode from originals.
                  </span>
                </span>
              </label>
            </fieldset>
          </div>
        )}

        {errorMessage && (
          <p className="mt-3 rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
            {errorMessage}
          </p>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            onClick={() => {
              if (!busy) onClose();
            }}
            disabled={busy}
            className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            onClick={() => void onConfirm()}
            disabled={busy || loading}
            className={clsx(
              "rounded-md px-3 py-1 text-sm",
              busy || loading
                ? "cursor-not-allowed bg-neutral-800 text-neutral-500"
                : "bg-sky-700 text-white hover:bg-sky-600",
            )}
          >
            {busy ? "Merging…" : "Merge"}
          </button>
        </div>
      </div>
    </div>
  );
}
