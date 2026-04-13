import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export function UpdateChecker() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState<"idle" | "downloading" | "error">(
    "idle",
  );
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    check()
      .then((u) => {
        if (u?.available) setUpdate(u);
      })
      .catch(() => {
        // Silently ignore update check failures (offline, no releases yet, etc.)
      });
  }, []);

  if (!update || dismissed) return null;

  const handleUpdate = async () => {
    try {
      setStatus("downloading");
      await update.downloadAndInstall();
      await relaunch();
    } catch {
      setStatus("error");
    }
  };

  return (
    <div className="fixed bottom-4 right-4 z-50 flex items-center gap-3 rounded-lg border border-neutral-700 bg-neutral-900 px-4 py-3 text-sm shadow-lg">
      {status === "error" ? (
        <>
          <span className="text-red-400">Update failed.</span>
          <button
            onClick={() => setDismissed(true)}
            className="text-neutral-500 hover:text-neutral-300"
          >
            Dismiss
          </button>
        </>
      ) : status === "downloading" ? (
        <span className="text-neutral-300">Downloading update...</span>
      ) : (
        <>
          <span className="text-neutral-300">
            v{update.version} available
          </span>
          <button
            onClick={handleUpdate}
            className="rounded bg-blue-600 px-3 py-1 text-xs font-medium text-white hover:bg-blue-500"
          >
            Update & Restart
          </button>
          <button
            onClick={() => setDismissed(true)}
            className="text-neutral-500 hover:text-neutral-300"
          >
            Later
          </button>
        </>
      )}
    </div>
  );
}
