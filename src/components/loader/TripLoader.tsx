import { useEffect, useState } from "react";
import { pickFolder } from "../../ipc/dialog";
import { scanFolder } from "../../ipc/scanner";
import { useStore } from "../../state/store";

const LAST_FOLDER_KEY = "tripviewer:lastFolder";

/**
 * Folder picker collapsed into a single clickable row that doubles as
 * the path display. Replaces the old "Open folder" blue button +
 * separate path label combo. Once a folder is chosen the picker still
 * works as the change-folder affordance.
 */
export function TripLoader() {
  const status = useStore((s) => s.status);
  const setStatus = useStore((s) => s.setStatus);
  const setError = useStore((s) => s.setError);
  const setScanResult = useStore((s) => s.setScanResult);
  const [lastPath, setLastPath] = useState<string | null>(
    () => localStorage.getItem(LAST_FOLDER_KEY),
  );

  async function loadFolder(folder: string) {
    setLastPath(folder);
    localStorage.setItem(LAST_FOLDER_KEY, folder);
    setStatus("loading");
    setError(null);
    try {
      const result = await scanFolder(folder);
      setScanResult(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function onPick() {
    const folder = await pickFolder();
    if (!folder) return;
    await loadFolder(folder);
  }

  useEffect(() => {
    if (lastPath && status === "idle") {
      loadFolder(lastPath);
    }
  }, []);

  const isLoading = status === "loading";
  const display = isLoading
    ? "Scanning…"
    : lastPath
      ? lastPath
      : "Open folder…";

  return (
    <button
      onClick={onPick}
      disabled={isLoading}
      title={lastPath ? `Click to change folder · current: ${lastPath}` : "Pick your dashcam library folder"}
      className="flex w-full items-center gap-2 rounded-md border border-neutral-800 bg-neutral-900 px-3 py-2 text-left text-xs text-neutral-300 transition-colors hover:border-neutral-700 hover:bg-neutral-800 disabled:cursor-not-allowed disabled:opacity-60"
    >
      <span className="shrink-0 text-neutral-500" aria-hidden="true">
        📁
      </span>
      <span className="flex-1 truncate">{display}</span>
      <span className="shrink-0 text-neutral-500" aria-hidden="true">
        ▾
      </span>
    </button>
  );
}
