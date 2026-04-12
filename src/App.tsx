import { useState } from "react";
import { TripLoader } from "./components/loader/TripLoader";
import { TripList } from "./components/loader/TripList";
import { PlayerShell } from "./components/video/PlayerShell";
import { useStore } from "./state/store";

function App() {
  const trips = useStore((s) => s.trips);
  const unmatched = useStore((s) => s.unmatched);
  const scanErrors = useStore((s) => s.scanErrors);
  const status = useStore((s) => s.status);
  const error = useStore((s) => s.error);
  const [showIssues, setShowIssues] = useState(false);

  const hasIssues = unmatched.length > 0 || scanErrors.length > 0;

  return (
    <div className="flex h-full">
      <aside className="flex w-72 flex-col border-r border-neutral-800">
        <header className="flex flex-col gap-3 border-b border-neutral-800 p-3">
          <h1 className="text-sm font-semibold tracking-tight">Trip Viewer</h1>
          <TripLoader />
          {status === "ready" && trips.length > 0 && (
            <div className="text-xs text-neutral-500">
              {trips.length} trips ·{" "}
              {trips.reduce((n, t) => n + t.segments.length, 0)} segments
              {hasIssues && (
                <button
                  onClick={() => setShowIssues(!showIssues)}
                  className="ml-1 text-yellow-500 hover:text-yellow-400"
                >
                  · {scanErrors.length + unmatched.length} issues{" "}
                  {showIssues ? "▾" : "▸"}
                </button>
              )}
            </div>
          )}
          {status === "ready" && trips.length === 0 && (
            <div className="rounded-md bg-yellow-950 px-2 py-1 text-xs text-yellow-300">
              No trips found in this folder. Check that it contains Wolf Box
              MP4 files with _F/_I/_R naming.
            </div>
          )}
          {error && (
            <div className="rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
              {error}
            </div>
          )}
          {showIssues && hasIssues && (
            <div className="max-h-40 overflow-y-auto rounded-md bg-neutral-900 p-2 text-[11px] text-neutral-400">
              {scanErrors.length > 0 && (
                <div className="mb-2">
                  <div className="font-semibold text-red-400">
                    {scanErrors.length} parse errors
                  </div>
                  {scanErrors.slice(0, 10).map((e, i) => (
                    <div key={i} className="truncate" title={e.reason}>
                      {e.path.split("\\").pop()} — {e.reason}
                    </div>
                  ))}
                  {scanErrors.length > 10 && (
                    <div className="text-neutral-500">
                      …and {scanErrors.length - 10} more
                    </div>
                  )}
                </div>
              )}
              {unmatched.length > 0 && (
                <div>
                  <div className="font-semibold text-yellow-400">
                    {unmatched.length} unmatched files
                  </div>
                  {unmatched.slice(0, 10).map((u, i) => (
                    <div key={i} className="truncate">
                      {u.split("\\").pop()}
                    </div>
                  ))}
                  {unmatched.length > 10 && (
                    <div className="text-neutral-500">
                      …and {unmatched.length - 10} more
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
        </header>
        <TripList />
      </aside>

      <main className="flex flex-1 flex-col">
        <PlayerShell />
      </main>
    </div>
  );
}

export default App;
