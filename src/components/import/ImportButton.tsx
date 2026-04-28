import { useStore } from "../../state/store";
import { discoverSources, startFolderImport } from "../../ipc/importer";
import { pickFolder } from "../../ipc/dialog";

const LAST_FOLDER_KEY = "tripviewer:lastFolder";

export function ImportButton() {
  const importStatus = useStore((s) => s.importStatus);
  const setImportStatus = useStore((s) => s.setImportStatus);
  const setImportSources = useStore((s) => s.setImportSources);
  const setImportError = useStore((s) => s.setImportError);
  const setImportRootPath = useStore((s) => s.setImportRootPath);

  const busy = importStatus !== "idle" && importStatus !== "complete" && importStatus !== "error";

  async function handleSdImport() {
    setImportStatus("discovering");
    try {
      const sources = await discoverSources();
      if (sources.length === 0) {
        setImportError("No dashcam SD cards detected. Insert an SD card and try again.");
        return;
      }
      setImportSources(sources);
      setImportStatus("confirming");
    } catch (e) {
      setImportError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleFolderImport() {
    const sourcePath = await pickFolder("Select the folder of files to import");
    if (!sourcePath) return;

    // Reuse the same library-root cache key the SD-card flow uses so
    // first-time users only pick a destination once across both flows.
    let rootPath = localStorage.getItem(LAST_FOLDER_KEY);
    if (!rootPath) {
      const chosen = await pickFolder("Select your dashcam library folder");
      if (!chosen) return;
      rootPath = chosen;
      localStorage.setItem(LAST_FOLDER_KEY, rootPath);
    }

    setImportRootPath(rootPath);
    // Folder import has no confirm step — the user already picked the
    // single source folder, there's nothing to deselect. Go straight
    // to running so the existing event listeners in ImportConfirmDialog
    // attach and the progress UI takes over.
    setImportStatus("running");
    try {
      await startFolderImport(rootPath, sourcePath);
    } catch (e) {
      setImportError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="flex flex-col items-start gap-1">
      <button
        onClick={handleSdImport}
        disabled={busy}
        className="rounded-md bg-neutral-700 px-4 py-2 text-sm font-medium text-neutral-200 transition-colors hover:bg-neutral-600 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {importStatus === "discovering" ? "Scanning…" : "Import from SD"}
      </button>
      <button
        onClick={handleFolderImport}
        disabled={busy}
        className="text-xs text-neutral-500 underline-offset-2 hover:text-neutral-300 hover:underline disabled:cursor-not-allowed disabled:opacity-50"
      >
        or import from a folder
      </button>
    </div>
  );
}
