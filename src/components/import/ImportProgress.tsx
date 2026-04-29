import { useStore } from "../../state/store";
import { cancelImport } from "../../ipc/importer";
import { formatBytes } from "../../utils/format";

function formatSpeed(bps: number): string {
  if (bps >= 1 << 20) return (bps / (1 << 20)).toFixed(1) + " MB/s";
  if (bps >= 1 << 10) return (bps / (1 << 10)).toFixed(1) + " KB/s";
  return bps.toFixed(0) + " B/s";
}

function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "";
  if (seconds >= 99 * 3600) return "~≥ 99h";
  const s = Math.round(seconds);
  if (s < 60) return `~${s}s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const rem = s % 60;
    return `~${m}m ${rem}s`;
  }
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return `~${h}h ${m}m`;
}

export function ImportProgress() {
  const importStatus = useStore((s) => s.importStatus);
  const phase = useStore((s) => s.importPhase);
  const progress = useStore((s) => s.importProgress);
  const warnings = useStore((s) => s.importWarnings);

  if (importStatus !== "running") return null;

  const pct =
    progress && progress.bytesTotal > 0
      ? (progress.bytesDone / progress.bytesTotal) * 100
      : progress && progress.filesTotal > 0
        ? (progress.filesDone / progress.filesTotal) * 100
        : 0;

  return (
    <div className="rounded-lg border border-neutral-700 bg-neutral-900 p-3 text-sm">
      {phase && (
        <div className="mb-2 text-xs font-medium text-cyan-400">
          {phase.message}
        </div>
      )}

      {progress && (
        <>
          <div className="mb-1 h-2 overflow-hidden rounded-full bg-neutral-800">
            <div
              className="h-full rounded-full bg-blue-500 transition-all duration-150"
              style={{ width: `${Math.min(pct, 100)}%` }}
            />
          </div>

          <div className="flex items-center justify-between text-xs text-neutral-400">
            <span>
              {progress.filesDone} / {progress.filesTotal} files
            </span>
            {progress.bytesTotal > 0 && (
              <span>
                {formatBytes(progress.bytesDone)} /{" "}
                {formatBytes(progress.bytesTotal)}
              </span>
            )}
            {progress.speedBps > 0 && (
              <span>{formatSpeed(progress.speedBps)}</span>
            )}
            {progress.phase === "staging" &&
              progress.speedBps > 0 &&
              progress.bytesTotal > 0 &&
              progress.bytesDone < progress.bytesTotal && (
                <span>
                  {formatEta(
                    (progress.bytesTotal - progress.bytesDone) /
                      progress.speedBps,
                  )}
                </span>
              )}
          </div>

          {progress.currentFile && (
            <div
              className="mt-1 truncate text-xs text-neutral-500"
              title={progress.currentFile}
            >
              {progress.currentFile}
            </div>
          )}
        </>
      )}

      {warnings.length > 0 && (
        <div className="mt-2 max-h-20 overflow-y-auto text-xs text-yellow-400">
          {warnings.map((w, i) => (
            <div key={i} className="truncate" title={w.message}>
              {w.message}
            </div>
          ))}
        </div>
      )}

      <button
        onClick={() => cancelImport()}
        className="mt-3 w-full rounded-md bg-red-900 px-3 py-1.5 text-xs font-medium text-red-200 hover:bg-red-800"
      >
        Cancel Import
      </button>
    </div>
  );
}
