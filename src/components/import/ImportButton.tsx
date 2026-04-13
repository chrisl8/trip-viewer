import { useStore } from "../../state/store";
import { discoverSources } from "../../ipc/importer";

export function ImportButton() {
  const importStatus = useStore((s) => s.importStatus);
  const setImportStatus = useStore((s) => s.setImportStatus);
  const setImportSources = useStore((s) => s.setImportSources);
  const setImportError = useStore((s) => s.setImportError);

  const busy = importStatus !== "idle" && importStatus !== "complete" && importStatus !== "error";

  async function handleClick() {
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

  return (
    <button
      onClick={handleClick}
      disabled={busy}
      className="rounded-md bg-neutral-700 px-4 py-2 text-sm font-medium text-neutral-200 transition-colors hover:bg-neutral-600 disabled:cursor-not-allowed disabled:opacity-50"
    >
      {importStatus === "discovering" ? "Scanning…" : "Import from SD"}
    </button>
  );
}
