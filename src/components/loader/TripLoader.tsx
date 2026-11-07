import { useEffect } from "react";
import { pickFolder } from "../../ipc/dialog";
import { reloadArchive, selectArchive } from "../../state/archiveNavigation";
import { useStore } from "../../state/store";

/**
 * Sidebar archive picker. Click to open a folder; the backend resolves
 * it to an archive root, opens (or creates) `<root>/.tripviewer/tripviewer.db`,
 * and persists `last_archive` so the next launch reopens automatically.
 *
 * Automatic scanning of restored and newly selected archives lives in
 * `useArchiveAutoScan`, which App calls unconditionally: this component
 * is unmounted while the sidebar is collapsed, and the startup splash
 * waits for that scan, so it must not depend on the sidebar being open.
 */
export function useArchiveAutoScan() {
  const archive = useStore((s) => s.currentArchive);
  useEffect(() => {
    if (archive) {
      void reloadArchive();
      void useStore.getState().refreshPlaces();
      void useStore.getState().refreshTimelapseJobs();
    }
  }, [archive]);
}

export function TripLoader() {
  const status = useStore((s) => s.status);
  const setError = useStore((s) => s.setError);
  const currentArchive = useStore((s) => s.currentArchive);

  async function onPick() {
    const folder = await pickFolder("Select your dashcam archive folder", currentArchive?.root);
    if (!folder) return;
    try {
      await selectArchive(folder);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

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
