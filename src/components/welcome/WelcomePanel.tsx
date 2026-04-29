import { useStore } from "../../state/store";

/**
 * Rendered by PlayerShell when no trip is loaded. Replaces the empty
 * VideoGrid + MapPanel that used to show a "No GPS data" dead column
 * and a placeholder "Select a trip…" message — neither of which gave
 * a new user any orientation.
 *
 * State-aware: copy adapts to whether a folder is open and whether
 * the library has any trips. The sidebar header already owns the
 * "no trips found in this folder" yellow banner (App.tsx); this panel
 * complements it with a short orientation card in the main pane.
 */
export function WelcomePanel() {
  const status = useStore((s) => s.status);
  const tripCount = useStore((s) => s.trips.length);

  return (
    <div className="flex h-full flex-col items-center justify-center bg-neutral-950 p-8 text-neutral-300">
      <div className="max-w-md rounded-lg border border-neutral-800 bg-neutral-900 p-6 shadow-lg">
        <h2 className="mb-3 text-base font-semibold text-neutral-100">
          Welcome to Trip Viewer
        </h2>
        {status === "loading" ? (
          <p className="text-sm text-neutral-400">Scanning folder…</p>
        ) : status !== "ready" || tripCount === 0 ? (
          <ol className="space-y-2 text-sm text-neutral-400">
            <li>
              <span className="font-medium text-neutral-200">1.</span> Click
              the folder path at the top of the sidebar to point Trip Viewer
              at your dashcam library.
            </li>
            <li>
              <span className="font-medium text-neutral-200">2.</span> Or
              click{" "}
              <span className="font-medium text-blue-400">
                Import from SD
              </span>{" "}
              (or{" "}
              <span className="text-neutral-300">import from a folder</span>)
              to copy fresh footage in. You&rsquo;ll be asked to pick a
              destination folder if you haven&rsquo;t already.
            </li>
            <li>
              <span className="font-medium text-neutral-200">3.</span> Once
              trips are loaded they&rsquo;ll appear in the list on the left
              — pick one to play it.
            </li>
          </ol>
        ) : (
          <ul className="space-y-2 text-sm text-neutral-400">
            <li>
              <span className="font-medium text-neutral-200">▶</span> Pick a
              trip on the left to play it.
            </li>
            <li>
              <span className="font-medium text-sky-300">Scan</span> tab —
              detect events, motion, audio silence, and known places.
            </li>
            <li>
              <span className="font-medium text-emerald-300">Review</span>{" "}
              tab — browse every segment with tags and bulk actions.
            </li>
            <li>
              <span className="font-medium text-violet-300">Timelapse</span>{" "}
              tab — pre-render fast-playback versions of each trip.
            </li>
          </ul>
        )}
      </div>
    </div>
  );
}
