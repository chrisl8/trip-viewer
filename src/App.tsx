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
import { ScanView } from "./components/scan/ScanView";
import { ReviewView } from "./components/review/ReviewView";
import { PlacesView } from "./components/places/PlacesView";
import { useStore } from "./state/store";
import { KIND_META, kindCounts } from "./utils/issueKinds";
import {
  onScanStart,
  onScanProgress,
  onScanDone,
} from "./ipc/scanner";

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
  const scanRunning = useStore((s) => s.scanRunning);
  const scanProgress = useStore((s) => s.scanProgress);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion);
    invoke<number>("get_video_port")
      .then((port) => setVideoPort(port))
      .catch((e) => console.error("get_video_port failed", e));
    void useStore.getState().loadUserApplicableTags();
    void useStore.getState().refreshPlaces();
  }, [setVideoPort]);

  // Attach scan-pipeline event listeners at the app root so progress
  // updates keep flowing even when the user navigates away from ScanView.
  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      onScanStart((e) => {
        useStore.setState({
          scanRunning: true,
          scanStartTotal: e.total,
          scanStartMs: Date.now(),
          scanProgress: {
            total: e.total,
            done: 0,
            failed: 0,
            currentSegmentId: null,
            currentScanId: null,
          },
          scanLastResult: null,
        });
      }),
    );
    unlisteners.push(
      onScanProgress((p) => {
        useStore.setState({ scanProgress: p });
      }),
    );
    unlisteners.push(
      onScanDone((result) => {
        useStore.setState({
          scanRunning: false,
          scanLastResult: result,
        });
        // Fresh tags landed — refresh sidebar badges and the selected
        // trip's per-segment tags if one is open.
        const state = useStore.getState();
        void state.refreshTripTagCounts();
        if (state.selectedTripId) {
          void state.refreshTripTags(state.selectedTripId);
        }
      }),
    );
    return () => {
      for (const p of unlisteners) {
        p.then((unlisten) => unlisten());
      }
    };
  }, []);

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
              <div className="mt-1 flex gap-1">
                <button
                  onClick={() =>
                    setMainView(mainView === "scan" ? "player" : "scan")
                  }
                  className={
                    mainView === "scan"
                      ? "rounded border border-sky-500 px-2 py-0.5 text-xs text-sky-300 hover:bg-neutral-800"
                      : "rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-300 hover:border-sky-500 hover:text-sky-300"
                  }
                  title={
                    mainView === "scan" ? "Close scan view" : "Open scan view"
                  }
                >
                  {scanRunning
                    ? `Scanning… ${scanProgress?.done ?? 0}/${scanProgress?.total ?? "?"}`
                    : "Scan"}
                </button>
                <button
                  onClick={() =>
                    setMainView(mainView === "review" ? "player" : "review")
                  }
                  className={
                    mainView === "review"
                      ? "rounded border border-emerald-500 px-2 py-0.5 text-xs text-emerald-300 hover:bg-neutral-800"
                      : "rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-300 hover:border-emerald-500 hover:text-emerald-300"
                  }
                  title={
                    mainView === "review"
                      ? "Close review view"
                      : "Open review view"
                  }
                >
                  Review
                </button>
                <button
                  onClick={() =>
                    setMainView(mainView === "places" ? "player" : "places")
                  }
                  className={
                    mainView === "places"
                      ? "rounded border border-rose-500 px-2 py-0.5 text-xs text-rose-300 hover:bg-neutral-800"
                      : "rounded border border-neutral-700 px-2 py-0.5 text-xs text-neutral-300 hover:border-rose-500 hover:text-rose-300"
                  }
                  title={
                    mainView === "places"
                      ? "Close places view"
                      : "Open places view"
                  }
                >
                  Places
                </button>
              </div>
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
        {mainView === "issues" ? (
          <IssuesView />
        ) : mainView === "scan" ? (
          <ScanView />
        ) : mainView === "review" ? (
          <ReviewView />
        ) : mainView === "places" ? (
          <PlacesView />
        ) : (
          <PlayerShell />
        )}
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
