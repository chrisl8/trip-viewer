import { useEffect, useState } from "react";
import { pickFolder } from "../../ipc/dialog";
import { scanFolder } from "../../ipc/scanner";
import { useStore } from "../../state/store";

const LAST_FOLDER_KEY = "tripviewer:lastFolder";

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

  return (
    <div className="flex flex-col gap-2">
      <button
        onClick={onPick}
        disabled={status === "loading"}
        className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {status === "loading" ? "Scanning…" : "Open folder"}
      </button>
      {lastPath && (
        <p className="truncate text-xs text-neutral-500" title={lastPath}>
          {lastPath}
        </p>
      )}
    </div>
  );
}
