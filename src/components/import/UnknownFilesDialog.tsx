import { useState } from "react";
import { useStore } from "../../state/store";
import { resolveUnknowns } from "../../ipc/importer";
import type { UnknownFileAction, UnknownFileDecision } from "../../types/import";
import { formatBytes } from "../../utils/format";

export function UnknownFilesDialog() {
  const importStatus = useStore((s) => s.importStatus);
  const unknowns = useStore((s) => s.importUnknowns);
  const setImportStatus = useStore((s) => s.setImportStatus);

  const [actions, setActions] = useState<Record<string, UnknownFileAction>>({});

  if (importStatus !== "paused_unknowns" || unknowns.length === 0) return null;

  function setAction(stagedPath: string, action: UnknownFileAction) {
    setActions((prev) => ({ ...prev, [stagedPath]: action }));
  }

  async function handleApply() {
    const decisions: UnknownFileDecision[] = unknowns.map((u) => ({
      stagedPath: u.stagedPath,
      action: actions[u.stagedPath] ?? "moveToOther",
    }));

    setImportStatus("running");
    await resolveUnknowns(decisions);
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-full max-w-lg rounded-lg border border-neutral-700 bg-neutral-900 p-6">
        <h2 className="mb-2 text-lg font-semibold text-neutral-100">
          Unknown Files
        </h2>
        <p className="mb-4 text-xs text-neutral-400">
          These files don't match known types. Choose what to do with each one.
        </p>

        <div className="mb-4 max-h-60 space-y-2 overflow-y-auto">
          {unknowns.map((u) => (
            <div
              key={u.stagedPath}
              className="flex items-center justify-between rounded-md bg-neutral-800 px-3 py-2"
            >
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm text-neutral-200" title={u.relPath}>
                  {u.filename}
                </div>
                <div className="text-xs text-neutral-500">
                  {u.extension || "no extension"} · {formatBytes(u.size)}
                </div>
              </div>
              <select
                value={actions[u.stagedPath] ?? "moveToOther"}
                onChange={(e) =>
                  setAction(u.stagedPath, e.target.value as UnknownFileAction)
                }
                className="ml-3 rounded bg-neutral-700 px-2 py-1 text-xs text-neutral-200"
              >
                <option value="moveToOther">Move to Other/</option>
                <option value="deleteFilename">
                  Delete (ignore this filename)
                </option>
                <option value="deleteExtension">
                  Delete (ignore all {u.extension})
                </option>
              </select>
            </div>
          ))}
        </div>

        <div className="flex justify-end">
          <button
            onClick={handleApply}
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-500"
          >
            Apply
          </button>
        </div>
      </div>
    </div>
  );
}
