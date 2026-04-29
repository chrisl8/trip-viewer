import { useEffect, useState } from "react";
import clsx from "clsx";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  clearTimelapseSettings,
  pickFfmpegBinary,
  testFfmpeg,
  type FfmpegCapabilities,
} from "../../ipc/timelapse";
import { useStore } from "../../state/store";

interface Props {
  onClose: () => void;
}

/**
 * First-run configuration for the opt-in ffmpeg dependency. The user
 * installs ffmpeg themselves (we suggest `winget install ffmpeg` on
 * Windows), then points this dialog at the binary. The Test button
 * runs `ffmpeg -version` and `-encoders` and caches the result.
 */
export function FfmpegConfig({ onClose }: Props) {
  const existingPath = useStore((s) => s.ffmpegPath);
  const existingCaps = useStore((s) => s.ffmpegCapabilities);
  const refreshSettings = useStore((s) => s.refreshTimelapseSettings);

  const [path, setPath] = useState(existingPath ?? "");
  const [caps, setCaps] = useState<FfmpegCapabilities | null>(existingCaps);
  const [testing, setTesting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Sync local `path` when the store value loads (or later changes).
  // `useState(existing)` only runs on first render — without this, the
  // text field stays empty if the modal mounted before settings loaded.
  // Guarded by checking the local path is still empty so we don't
  // clobber in-flight user edits.
  useEffect(() => {
    if (existingPath && path === "") setPath(existingPath);
  }, [existingPath, path]);

  // Same deal for capabilities: pick them up when the store populates.
  useEffect(() => {
    if (existingCaps && !caps) setCaps(existingCaps);
  }, [existingCaps, caps]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function onBrowse() {
    try {
      const picked = await pickFfmpegBinary();
      if (picked) {
        setPath(picked);
        // Auto-test after a successful browse so the happy path is
        // one click: Browse → done.
        await runTest(picked);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function runTest(p: string) {
    setTesting(true);
    setError(null);
    try {
      const c = await testFfmpeg(p);
      setCaps(c);
      await refreshSettings();
    } catch (e) {
      setCaps(null);
      setError(String(e));
    } finally {
      setTesting(false);
    }
  }

  async function onClear() {
    setError(null);
    try {
      await clearTimelapseSettings();
      setPath("");
      setCaps(null);
      await refreshSettings();
    } catch (e) {
      setError(String(e));
    }
  }

  const canTest = path.trim().length > 0 && !testing;
  // Only offer Clear when there's actually something to clear. Reading
  // from the store (not local state) so the button reflects what's
  // persisted, not in-flight edits.
  const canClear = (existingPath !== null || existingCaps !== null) && !testing;

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-[32rem] rounded-md border border-neutral-700 bg-neutral-900 p-5 text-neutral-100"
      >
        <h2 className="text-base font-semibold">Configure ffmpeg</h2>
        <p className="mt-1 text-xs text-neutral-400">
          Timelapse generation uses an ffmpeg binary you install yourself,
          so the app bundle stays small. One-time setup.
        </p>

        <div className="mt-4 flex flex-col gap-1">
          <label className="text-xs text-neutral-400">
            Path to ffmpeg executable
          </label>
          <div className="flex gap-2">
            <input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="C:\Program Files\ffmpeg\bin\ffmpeg.exe"
              className="flex-1 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-sky-500 focus:outline-none"
            />
            <button
              onClick={() => void onBrowse()}
              disabled={testing}
              className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
            >
              Browse…
            </button>
          </div>
        </div>

        <div className="mt-3 flex items-center gap-2">
          <button
            onClick={() => void runTest(path)}
            disabled={!canTest}
            className={clsx(
              "rounded-md px-3 py-1 text-sm",
              canTest
                ? "bg-sky-700 text-white hover:bg-sky-600"
                : "cursor-not-allowed bg-neutral-800 text-neutral-500",
            )}
          >
            {testing ? "Testing…" : "Test"}
          </button>
          <button
            onClick={() => void onClear()}
            disabled={!canClear}
            title="Erase the saved ffmpeg path and capabilities. Timelapse encoding will be disabled until you point at a binary again."
            className={clsx(
              "rounded-md px-3 py-1 text-sm",
              canClear
                ? "border border-neutral-700 text-neutral-300 hover:bg-neutral-800"
                : "cursor-not-allowed border border-neutral-800 text-neutral-600",
            )}
          >
            Clear
          </button>
          {caps && !testing && (
            <div className="text-xs">
              <span className="text-emerald-400">✓ {caps.version}</span>
              <span className="ml-2 text-neutral-400">
                {caps.nvencHevc
                  ? "· NVENC available (fast GPU encoding)"
                  : "· NVENC not found (will use CPU encoding)"}
              </span>
            </div>
          )}
        </div>

        {error && (
          <div className="mt-3 rounded-md bg-red-950 px-3 py-2 text-xs text-red-300">
            {error}
          </div>
        )}

        <div className="mt-5 rounded-md border border-neutral-800 bg-neutral-950 p-3 text-xs text-neutral-400">
          <div className="mb-1 font-medium text-neutral-300">
            Don&apos;t have ffmpeg?
          </div>
          <div>
            On Windows, open a terminal and run:{" "}
            <code className="rounded bg-neutral-800 px-1.5 py-0.5 text-neutral-200">
              winget install ffmpeg
            </code>
            . It typically installs to{" "}
            <code className="text-neutral-300">
              %LOCALAPPDATA%\Microsoft\WinGet\Packages\…\ffmpeg.exe
            </code>
            .
          </div>
          <button
            onClick={() =>
              void openUrl("https://ffmpeg.org/download.html")
            }
            className="mt-2 text-sky-400 underline hover:text-sky-300"
          >
            Other platforms / manual download →
          </button>
        </div>

        <div className="mt-5 flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-md border border-neutral-700 px-3 py-1 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
