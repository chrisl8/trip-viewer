import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { TripLoader } from "./components/loader/TripLoader";
import { TripList } from "./components/loader/TripList";
import { HevcSupportGate } from "./components/video/HevcSupportGate";
import { PlayerShell } from "./components/video/PlayerShell";
import { UpdateChecker } from "./components/UpdateChecker";
import { KeyboardShortcutsHelp } from "./components/KeyboardShortcutsHelp";
import { ImportButton } from "./components/import/ImportButton";
import { ImportConfirmDialog } from "./components/import/ImportConfirmDialog";
import { ImportProgress } from "./components/import/ImportProgress";
import { UnknownFilesDialog } from "./components/import/UnknownFilesDialog";
import { ImportSummary } from "./components/import/ImportSummary";
import { useStore } from "./state/store";

function App() {
  const trips = useStore((s) => s.trips);
  const unmatched = useStore((s) => s.unmatched);
  const scanErrors = useStore((s) => s.scanErrors);
  const status = useStore((s) => s.status);
  const error = useStore((s) => s.error);
  const importError = useStore((s) => s.importError);
  const resetImport = useStore((s) => s.resetImport);
  const [showIssues, setShowIssues] = useState(false);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion);
  }, []);

  const hasIssues = unmatched.length > 0 || scanErrors.length > 0;

  return (
    <HevcSupportGate>
    <div className="flex h-full">
      <aside className="flex w-72 flex-col border-r border-neutral-800">
        <header className="flex flex-col gap-3 border-b border-neutral-800 p-3">
          <h1 className="text-sm font-semibold tracking-tight">Trip Viewer</h1>
          <TripLoader />
          <ImportButton />
          {importError && (
            <div className="flex items-start gap-2 rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
              <span className="flex-1">{importError}</span>
              <button onClick={resetImport} className="shrink-0 text-red-500 hover:text-red-300">
                ×
              </button>
            </div>
          )}
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
        <ImportProgress />
        <TripList />
        <footer className="flex items-center justify-between border-t border-neutral-800 px-3 py-2.5">
          <span className="text-xs text-neutral-500">v{version}</span>
          <button
            onClick={() => setShowShortcuts(true)}
            className="text-xs text-neutral-400 hover:text-neutral-200"
          >
            Keyboard shortcuts
          </button>
        </footer>
      </aside>

      <main className="flex flex-1 flex-col">
        <PlayerShell />
      </main>
    </div>
    {showShortcuts && (
      <KeyboardShortcutsHelp onClose={() => setShowShortcuts(false)} />
    )}
    <ImportConfirmDialog />
    <UnknownFilesDialog />
    <ImportSummary />
    <UpdateChecker />
    </HevcSupportGate>
  );
}

export default App;
