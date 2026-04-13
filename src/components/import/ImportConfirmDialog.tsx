import { useEffect } from "react";
import { useStore } from "../../state/store";
import { pickFolder } from "../../ipc/dialog";
import {
  startImport,
  onImportPhase,
  onImportProgress,
  onImportWarning,
  onImportUnknowns,
  onImportComplete,
} from "../../ipc/importer";
import type { UnlistenFn } from "@tauri-apps/api/event";

const LAST_FOLDER_KEY = "tripviewer:lastFolder";

function formatBytes(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(1) + " GB";
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " MB";
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + " KB";
  return bytes + " B";
}

export function ImportConfirmDialog() {
  const importStatus = useStore((s) => s.importStatus);
  const sources = useStore((s) => s.importSources);
  const setImportStatus = useStore((s) => s.setImportStatus);
  const setImportError = useStore((s) => s.setImportError);
  const resetImport = useStore((s) => s.resetImport);

  // Set up event listeners when import starts
  useEffect(() => {
    if (importStatus !== "running") return;

    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      onImportPhase((phase) => {
        useStore.getState().setImportPhase(phase);
      }),
    );
    unlisteners.push(
      onImportProgress((progress) => {
        useStore.getState().setImportProgress(progress);
      }),
    );
    unlisteners.push(
      onImportWarning((warning) => {
        useStore.getState().addImportWarning(warning);
      }),
    );
    unlisteners.push(
      onImportUnknowns((unknowns) => {
        useStore.getState().setImportUnknowns(unknowns);
      }),
    );
    unlisteners.push(
      onImportComplete((result) => {
        useStore.getState().setImportResult(result);
      }),
    );

    return () => {
      for (const p of unlisteners) {
        p.then((unlisten) => unlisten());
      }
    };
  }, [importStatus]);

  if (importStatus !== "confirming") return null;

  async function handleStart() {
    let rootPath = localStorage.getItem(LAST_FOLDER_KEY);

    // First-time user: no folder open yet — ask where to store files
    if (!rootPath) {
      const chosen = await pickFolder();
      if (!chosen) return; // User cancelled the picker
      rootPath = chosen;
    }

    useStore.getState().setImportRootPath(rootPath);
    setImportStatus("running");
    try {
      await startImport(rootPath, sources);
    } catch (e) {
      setImportError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-full max-w-md rounded-lg border border-neutral-700 bg-neutral-900 p-6">
        <h2 className="mb-4 text-lg font-semibold text-neutral-100">
          Import from SD Card
        </h2>

        <div className="mb-4 space-y-2">
          {sources.map((src) => (
            <div
              key={src.label}
              className="flex items-center justify-between rounded-md bg-neutral-800 px-3 py-2 text-sm"
            >
              <div>
                <span className="font-medium text-neutral-200">
                  {src.label.toUpperCase()}
                </span>
                <span className="ml-2 text-neutral-400">{src.path}</span>
                {src.readOnly && (
                  <span className="ml-2 rounded bg-yellow-900 px-1.5 py-0.5 text-xs text-yellow-300">
                    Read-only
                  </span>
                )}
              </div>
              <div className="text-xs text-neutral-500">
                {src.fileCount} files · {formatBytes(src.totalBytes)}
              </div>
            </div>
          ))}
        </div>

        <div className="flex justify-end gap-2">
          <button
            onClick={resetImport}
            className="rounded-md px-4 py-2 text-sm text-neutral-400 hover:text-neutral-200"
          >
            Cancel
          </button>
          <button
            onClick={handleStart}
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-500"
          >
            Start Import
          </button>
        </div>
      </div>
    </div>
  );
}
