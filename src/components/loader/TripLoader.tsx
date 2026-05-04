import { useEffect } from "react";
import { openArchive } from "../../ipc/archive";
import { pickFolder } from "../../ipc/dialog";
import { scanFolder } from "../../ipc/scanner";
import { useStore } from "../../state/store";

/**
 * Sidebar archive picker. Click to open a folder; the backend resolves
 * it to an archive root, opens (or creates) `<root>/.tripviewer/tripviewer.db`,
 * and persists `last_archive` so the next launch reopens automatically.
 *
 * On mount, if the backend already has an archive open (auto-reopened
 * from `last_archive`), we kick off the initial scan so the sidebar
 * populates without an extra click.
 */
export function TripLoader() {
  const status = useStore((s) => s.status);
  const setStatus = useStore((s) => s.setStatus);
  const setError = useStore((s) => s.setError);
  const setScanResult = useStore((s) => s.setScanResult);
  const currentArchive = useStore((s) => s.currentArchive);
  const setCurrentArchive = useStore((s) => s.setCurrentArchive);

  async function scanCurrent(folder: string) {
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
    try {
      const info = await openArchive(folder);
      setCurrentArchive(info);
      await scanCurrent(info.root);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  // Auto-scan on mount when the backend already had an archive open
  // (typical case: user reopened the app, last_archive was reachable).
  useEffect(() => {
    if (currentArchive && status === "idle") {
      void scanCurrent(currentArchive.root);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentArchive?.root]);

  const isLoading = status === "loading";
  const display = isLoading
    ? "Scanning…"
    : currentArchive
      ? currentArchive.label
      : "Open archive…";

  return (
    <button
      onClick={onPick}
      disabled={isLoading}
      title={
        currentArchive
          ? `Click to switch archive · current: ${currentArchive.root}`
          : "Pick your dashcam archive folder"
      }
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
