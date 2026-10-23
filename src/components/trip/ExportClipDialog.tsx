import { useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useStore } from "../../state/store";
import {
  cancelExport,
  onExportDone,
  onExportProgress,
  pickExportPath,
  startExport,
  type ExportDoneEvent,
  type ExportLayout,
  type ExportPipEdge,
  type ExportCodec,
  type ExportMapStyle,
  type ExportQuality,
  type ExportSlotSize,
} from "../../ipc/export";
import type { Trip } from "../../types/model";
import { computeTripTime } from "../../utils/tripTime";
import {
  audioArgsFromPercent,
  defaultClipFileName,
  formatClock,
  parseClockInput,
  tripChannelLabels,
  tripDurationS,
  tripSegmentsForExport,
  validateRange,
} from "../../utils/exportClip";

interface Props {
  trip: Trip;
  onClose: () => void;
}

type Phase = "form" | "running" | "done";

const inputCls =
  "rounded border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 focus:border-neutral-500 focus:outline-none";
const labelCls = "text-xs uppercase tracking-wide text-neutral-500";

/**
 * "Export clip…" dialog: renders a stretch of a trip as one MP4 with the
 * camera's channels combined — the main channel full size and the others
 * small beside it (or on top of it), optionally with a GPS route map.
 * The encoding itself runs in the backend through the user's ffmpeg.
 */
export function ExportClipDialog({ trip, onClose }: Props) {
  const ffmpegPath = useStore((s) => s.ffmpegPath);
  const loadedTripId = useStore((s) => s.loadedTripId);
  const activeSegmentId = useStore((s) => s.activeSegmentId);
  const currentTime = useStore((s) => s.currentTime);
  const sourceMode = useStore((s) => s.sourceMode);

  const labels = useMemo(() => tripChannelLabels(trip), [trip]);
  const totalS = useMemo(() => tripDurationS(trip), [trip]);

  // Start where the player is, when this trip is the one being watched.
  const initialStart = useMemo(() => {
    if (loadedTripId !== trip.id || sourceMode !== "original") return 0;
    const t = computeTripTime(
      trip,
      activeSegmentId,
      currentTime,
      "original",
      null,
    );
    return Math.max(0, Math.min(Math.floor(t), Math.max(0, totalS - 1)));
    // Only the value at open time matters; don't follow playback afterwards.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const initialMain = labels.includes("Front") ? "Front" : (labels[0] ?? "");

  const [startText, setStartText] = useState(formatClock(initialStart));
  const [lengthText, setLengthText] = useState("30");
  const [layout, setLayout] = useState<ExportLayout>("side");
  const [mainLabel, setMainLabel] = useState(initialMain);
  const [hidden, setHidden] = useState<Record<string, boolean>>({});
  const [slotSize, setSlotSize] = useState<ExportSlotSize>("medium");
  const [pipEdge, setPipEdge] = useState<ExportPipEdge>("top");
  const [quality, setQuality] = useState<ExportQuality>("balanced");
  const [codec, setCodec] = useState<ExportCodec>("h264");
  const [showMap, setShowMap] = useState(trip.gpsSupported);
  const [mapStyle, setMapStyle] = useState<ExportMapStyle>("route");
  const [volumes, setVolumes] = useState<Record<string, number>>(() =>
    initialMain ? { [initialMain]: 100 } : {},
  );

  const [phase, setPhase] = useState<Phase>("form");
  const [fraction, setFraction] = useState(0);
  const [result, setResult] = useState<ExportDoneEvent | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const range = validateRange(
    parseClockInput(startText),
    parseClockInput(lengthText),
    totalS,
  );
  const others = labels.filter((l) => l !== mainLabel);
  const slotLabels = others.filter((l) => !hidden[l]);
  const canExport = phase === "form" && ffmpegPath != null && range != null;

  useEffect(() => {
    const unlistenProgress = onExportProgress((e) => setFraction(e.fraction));
    const unlistenDone = onExportDone((e) => {
      setResult(e);
      setPhase(e.cancelled ? "form" : "done");
    });
    return () => {
      void unlistenProgress.then((u) => u());
      void unlistenDone.then((u) => u());
    };
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && phase !== "running") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose, phase]);

  async function onExport() {
    if (!range) return;
    setErrorMessage(null);
    const outputPath = await pickExportPath(
      defaultClipFileName(trip, range.startS),
    );
    if (!outputPath) return;
    setFraction(0);
    setResult(null);
    setPhase("running");
    try {
      await startExport({
        outputPath,
        segments: tripSegmentsForExport(trip),
        cameraKind: trip.cameraKind,
        startS: range.startS,
        durationS: range.durationS,
        layout,
        slotSize,
        pipEdge,
        quality,
        codec,
        mainLabel,
        slotLabels,
        showMap: showMap && trip.gpsSupported,
        mapStyle,
        audio: audioArgsFromPercent(volumes),
      });
    } catch (e) {
      setPhase("form");
      setErrorMessage(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
      onClick={() => {
        if (phase !== "running") onClose();
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="max-h-[90vh] w-[34rem] overflow-y-auto rounded-md border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
      >
        <h2 className="text-base font-semibold">Export clip</h2>
        <p className="mt-1 text-sm text-neutral-400">
          One video with all channels together. Trip length{" "}
          {formatClock(totalS)}.
        </p>

        {ffmpegPath == null && (
          <p className="mt-3 rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300">
            Export needs ffmpeg. Set its path in the Timelapse tab first.
          </p>
        )}

        {phase === "form" && (
          <div className="mt-4 space-y-4">
            <div className="flex gap-4">
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Start (m:ss)</span>
                <input
                  className={clsx(inputCls, "w-28")}
                  value={startText}
                  onChange={(e) => setStartText(e.target.value)}
                />
              </label>
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Length (seconds or m:ss)</span>
                <input
                  className={clsx(inputCls, "w-28")}
                  value={lengthText}
                  onChange={(e) => setLengthText(e.target.value)}
                />
              </label>
            </div>
            {range == null && (
              <p className="text-xs text-red-300">
                Enter a start inside the trip and a length of at least 1 second.
              </p>
            )}

            <div className="flex flex-col gap-1">
              <span className={labelCls}>Layout</span>
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="radio"
                  checked={layout === "side"}
                  onChange={() => setLayout("side")}
                  className="mt-1"
                />
                <span>
                  Side column
                  <span className="block text-xs text-neutral-500">
                    Main video untouched, the others stacked to its right.
                    Nothing is covered.
                  </span>
                </span>
              </label>
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="radio"
                  checked={layout === "pip"}
                  onChange={() => setLayout("pip")}
                  className="mt-1"
                />
                <span>
                  Picture-in-picture
                  <span className="block text-xs text-neutral-500">
                    The others sit over the main video, along its top or bottom
                    edge.
                  </span>
                </span>
              </label>
            </div>

            <div className="flex flex-wrap gap-4">
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Main camera</span>
                <select
                  className={inputCls}
                  value={mainLabel}
                  onChange={(e) => setMainLabel(e.target.value)}
                >
                  {labels.map((l) => (
                    <option key={l} value={l}>
                      {l}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Small tiles</span>
                <select
                  className={inputCls}
                  value={slotSize}
                  onChange={(e) =>
                    setSlotSize(e.target.value as ExportSlotSize)
                  }
                >
                  <option value="small">Small</option>
                  <option value="medium">Medium</option>
                  <option value="large">Large</option>
                </select>
              </label>
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Quality</span>
                <select
                  className={inputCls}
                  value={quality}
                  onChange={(e) => setQuality(e.target.value as ExportQuality)}
                >
                  <option value="high">High (large file)</option>
                  <option value="balanced">Balanced</option>
                  <option value="small">Small file (messaging)</option>
                </select>
              </label>
              <label className="flex flex-col gap-1">
                <span className={labelCls}>Format</span>
                <select
                  className={inputCls}
                  value={codec}
                  onChange={(e) => setCodec(e.target.value as ExportCodec)}
                  title="H.265 makes smaller files but encodes more slowly and plays on fewer devices."
                >
                  <option value="h264">H.264 (plays everywhere)</option>
                  <option value="h265">H.265 (smaller, slower)</option>
                </select>
              </label>
              {layout === "pip" && (
                <label className="flex flex-col gap-1">
                  <span className={labelCls}>Placed at</span>
                  <select
                    className={inputCls}
                    value={pipEdge}
                    onChange={(e) =>
                      setPipEdge(e.target.value as ExportPipEdge)
                    }
                  >
                    <option value="top">Top</option>
                    <option value="bottom">Bottom</option>
                  </select>
                </label>
              )}
            </div>

            <div className="flex flex-col gap-1">
              <span className={labelCls}>Also show</span>
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {others.map((l) => (
                  <label key={l} className="flex items-center gap-1.5 text-sm">
                    <input
                      type="checkbox"
                      checked={!hidden[l]}
                      onChange={(e) =>
                        setHidden((h) => ({ ...h, [l]: !e.target.checked }))
                      }
                    />
                    {l}
                  </label>
                ))}
                <label
                  className={clsx(
                    "flex items-center gap-1.5 text-sm",
                    !trip.gpsSupported && "text-neutral-600",
                  )}
                >
                  <input
                    type="checkbox"
                    disabled={!trip.gpsSupported}
                    checked={showMap && trip.gpsSupported}
                    onChange={(e) => setShowMap(e.target.checked)}
                  />
                  GPS route map
                </label>
              </div>
              {showMap && trip.gpsSupported && (
                <label className="flex items-center gap-2 text-sm">
                  <span className="text-neutral-400">Map background</span>
                  <select
                    className={inputCls}
                    value={mapStyle}
                    onChange={(e) =>
                      setMapStyle(e.target.value as ExportMapStyle)
                    }
                  >
                    <option value="route">Route only (works offline)</option>
                    <option value="streets">Street map (needs internet)</option>
                  </select>
                </label>
              )}
              {showMap && trip.gpsSupported && mapStyle === "streets" && (
                <p className="text-xs text-neutral-500">
                  Downloads a few OpenStreetMap tiles for this export (nothing
                  is kept afterwards) and credits OpenStreetMap contributors in
                  the picture. Falls back to the route only if it can&apos;t
                  connect.
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1">
              <span className={labelCls}>Sound (volume per camera)</span>
              {labels.map((l) => {
                const v = volumes[l] ?? 0;
                return (
                  <label key={l} className="flex items-center gap-3 text-sm">
                    <span className="w-20">{l}</span>
                    <input
                      type="range"
                      min={0}
                      max={200}
                      step={10}
                      value={v}
                      onChange={(e) =>
                        setVolumes((all) => ({
                          ...all,
                          [l]: Number(e.target.value),
                        }))
                      }
                      className="flex-1"
                    />
                    <span className="w-12 text-right text-xs text-neutral-400">
                      {v === 0 ? "off" : `${v}%`}
                    </span>
                  </label>
                );
              })}
              <span className="text-xs text-neutral-500">
                Cameras with a single microphone record the same sound on every
                channel, so use just one. Several are mixed.
              </span>
            </div>

            {errorMessage && (
              <p className="rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
                {errorMessage}
              </p>
            )}
            {result?.cancelled && (
              <p className="text-xs text-neutral-400">Export cancelled.</p>
            )}
          </div>
        )}

        {phase === "running" && (
          <div className="mt-4">
            <div className="h-2 w-full overflow-hidden rounded bg-neutral-800">
              <div
                className="h-full bg-sky-500 transition-[width]"
                style={{ width: `${Math.round(fraction * 100)}%` }}
              />
            </div>
            <p className="mt-2 text-sm text-neutral-300">
              Exporting… {Math.round(fraction * 100)}%
            </p>
            <p className="mt-1 text-xs text-neutral-500">
              Long clips and 4K footage take a while. You can keep using the
              app.
            </p>
          </div>
        )}

        {phase === "done" && result && (
          <div className="mt-4 space-y-2">
            {result.ok ? (
              <>
                <p className="text-sm text-emerald-300">Clip exported.</p>
                <p className="break-all text-xs text-neutral-400">
                  {result.outputPath}
                </p>
                {result.warnings.map((w) => (
                  <p
                    key={w}
                    className="rounded-md bg-amber-950 px-2 py-1 text-xs text-amber-300"
                  >
                    {w}
                  </p>
                ))}
              </>
            ) : (
              <p className="rounded-md bg-red-950 px-2 py-1 text-xs text-red-300">
                {result.error ?? "Export failed."}
              </p>
            )}
          </div>
        )}

        <div className="mt-5 flex justify-end gap-2">
          {phase === "form" && (
            <>
              <button
                onClick={onClose}
                className="rounded px-3 py-1.5 text-sm text-neutral-300 hover:bg-neutral-800"
              >
                Close
              </button>
              <button
                disabled={!canExport}
                onClick={() => void onExport()}
                className={clsx(
                  "rounded px-3 py-1.5 text-sm font-medium",
                  canExport
                    ? "bg-sky-600 text-white hover:bg-sky-500"
                    : "bg-neutral-800 text-neutral-500",
                )}
              >
                Choose file and export…
              </button>
            </>
          )}
          {phase === "running" && (
            <button
              onClick={() => void cancelExport()}
              className="rounded px-3 py-1.5 text-sm text-neutral-300 hover:bg-neutral-800"
            >
              Cancel export
            </button>
          )}
          {phase === "done" && result && (
            <>
              {!result.ok && (
                <button
                  onClick={() => setPhase("form")}
                  className="rounded px-3 py-1.5 text-sm text-neutral-300 hover:bg-neutral-800"
                >
                  Back
                </button>
              )}
              {result.ok && result.outputPath && (
                <button
                  onClick={() => {
                    if (result.outputPath) {
                      void revealItemInDir(result.outputPath).catch((err) =>
                        console.error("revealItemInDir failed", err),
                      );
                    }
                  }}
                  className="rounded px-3 py-1.5 text-sm text-neutral-300 hover:bg-neutral-800"
                >
                  Show in folder
                </button>
              )}
              <button
                onClick={onClose}
                className="rounded bg-neutral-800 px-3 py-1.5 text-sm text-neutral-100 hover:bg-neutral-700"
              >
                Done
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
