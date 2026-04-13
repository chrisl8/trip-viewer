import { useStore } from "../../state/store";
import { scanFolder } from "../../ipc/scanner";

function formatBytes(bytes: number): string {
  if (bytes >= 1 << 30) return (bytes / (1 << 30)).toFixed(1) + " GB";
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + " MB";
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + " KB";
  return bytes + " B";
}

export function ImportSummary() {
  const importStatus = useStore((s) => s.importStatus);
  const result = useStore((s) => s.importResult);
  const resetImport = useStore((s) => s.resetImport);
  const setScanResult = useStore((s) => s.setScanResult);

  if (importStatus !== "complete" || !result) return null;

  async function handleClose() {
    // Auto-rescan the folder so new trips appear
    const rootPath = localStorage.getItem("tripviewer:lastFolder");
    if (rootPath) {
      try {
        const scanResult = await scanFolder(rootPath);
        setScanResult(scanResult);
      } catch {
        // Ignore scan errors on close
      }
    }
    resetImport();
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-full max-w-md rounded-lg border border-neutral-700 bg-neutral-900 p-6">
        <h2 className="mb-4 text-lg font-semibold text-neutral-100">
          Import Complete
        </h2>

        <div className="space-y-3">
          {result.sources.map((src) => (
            <div
              key={src.sourceLabel || "default"}
              className="rounded-md bg-neutral-800 p-3 text-sm"
            >
              {src.sourceLabel && (
                <div className="mb-2 font-medium text-neutral-200">
                  {src.sourceLabel.toUpperCase()}
                  {src.readOnly && (
                    <span className="ml-2 text-xs text-yellow-400">
                      read-only
                    </span>
                  )}
                </div>
              )}

              {src.error ? (
                <div className="text-red-400">{src.error}</div>
              ) : src.noFiles ? (
                <div className="text-neutral-500">No files found</div>
              ) : (
                <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                  <span className="text-neutral-400">Files staged</span>
                  <span className="text-neutral-200">
                    {src.filesStaged} ({formatBytes(src.bytesStaged)})
                  </span>
                  <span className="text-neutral-400">Videos moved</span>
                  <span className="text-neutral-200">{src.videosMoved}</span>
                  <span className="text-neutral-400">Photos moved</span>
                  <span className="text-neutral-200">{src.photosMoved}</span>
                  {src.dupsSkipped > 0 && (
                    <>
                      <span className="text-neutral-400">Duplicates skipped</span>
                      <span className="text-neutral-200">{src.dupsSkipped}</span>
                    </>
                  )}
                  {src.unknownFiles > 0 && (
                    <>
                      <span className="text-neutral-400">Unknown files</span>
                      <span className="text-neutral-200">{src.unknownFiles}</span>
                    </>
                  )}
                  {(src.earliestDate || src.latestDate) && (
                    <>
                      <span className="text-neutral-400">Date range</span>
                      <span className="text-neutral-200">
                        {src.earliestDate === src.latestDate
                          ? src.earliestDate
                          : `${src.earliestDate} — ${src.latestDate}`}
                      </span>
                    </>
                  )}
                  <span className="text-neutral-400">Source wiped</span>
                  <span className={src.sourceWiped ? "text-green-400" : "text-yellow-400"}>
                    {src.sourceWiped ? "Yes" : "No"}
                  </span>
                </div>
              )}

              {src.warnings.length > 0 && (
                <div className="mt-2 space-y-1">
                  {src.warnings.map((w, i) => (
                    <div key={i} className="text-xs text-yellow-400">
                      {w}
                    </div>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>

        {result.logPath && (
          <div className="mt-3 truncate text-xs text-neutral-500" title={result.logPath}>
            Log: {result.logPath}
          </div>
        )}

        <div className="mt-4 flex justify-end">
          <button
            onClick={handleClose}
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-500"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
