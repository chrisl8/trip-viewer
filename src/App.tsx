import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
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
import { IssuesView } from "./components/issues/IssuesView";
import { useStore } from "./state/store";
import { KIND_META, kindCounts } from "./utils/issueKinds";

function App() {
  const trips = useStore((s) => s.trips);
  const scanErrors = useStore((s) => s.scanErrors);
  const status = useStore((s) => s.status);
  const error = useStore((s) => s.error);
  const importError = useStore((s) => s.importError);
  const resetImport = useStore((s) => s.resetImport);
  const setVideoPort = useStore((s) => s.setVideoPort);
  const mainView = useStore((s) => s.mainView);
  const setMainView = useStore((s) => s.setMainView);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion);
    invoke<number>("get_video_port")
      .then((port) => setVideoPort(port))
      .catch((e) => console.error("get_video_port failed", e));
  }, [setVideoPort]);

  const issueCount = scanErrors.length;
  const issuesOpen = mainView === "issues";
  const issueBreakdown = kindCounts(scanErrors);

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
            <div className="flex flex-col gap-0.5 text-xs text-neutral-500">
              <div>
                {trips.length} trips ·{" "}
                {trips.reduce((n, t) => n + t.segments.length, 0)} segments
                {issueCount > 0 && (
                  <button
                    onClick={() => setMainView(issuesOpen ? "player" : "issues")}
                    className={
                      issuesOpen
                        ? "ml-1 text-yellow-300 hover:text-yellow-200"
                        : "ml-1 text-yellow-500 hover:text-yellow-400"
                    }
                    title={issuesOpen ? "Close issues view" : "Open issues view"}
                  >
                    · {issueCount} {issueCount === 1 ? "issue" : "issues"}{" "}
                    {issuesOpen ? "◧" : "▸"}
                  </button>
                )}
              </div>
              {issueCount > 0 && issueBreakdown.length > 0 && (
                <div className="flex flex-wrap gap-x-2 text-[11px] text-neutral-600">
                  {issueBreakdown.slice(0, 3).map(({ kind, count }) => (
                    <span key={kind}>
                      {count} {KIND_META[kind].label.toLowerCase()}
                    </span>
                  ))}
                  {issueBreakdown.length > 3 && (
                    <span>+{issueBreakdown.length - 3} more</span>
                  )}
                </div>
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
        {mainView === "issues" ? <IssuesView /> : <PlayerShell />}
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
