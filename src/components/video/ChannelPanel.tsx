import {
  forwardRef,
  type MouseEvent as ReactMouseEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import clsx from "clsx";
import {
  loadMediaHealth,
  recordMediaFailure,
  recordMediaRecovery,
  type MediaHealthRange,
} from "../../ipc/mediaHealth";
import { useStore } from "../../state/store";
import { usePausedZoom } from "./usePausedZoom";
import { useOverlayCrop } from "./useOverlayCrop";
import type { OverlayCrop, OverlayFrame } from "../../settings/overlayLayout";
import { shouldMuteChannelAudio } from "./audioPolicy";

// Diagnostic toggles. Both default off so production builds stay silent.
//
//   DEBUG_MEDIA          — once-per-second summary per channel: media time,
//                          real-time ratio, readyState, buffered window, and
//                          event counts since the last tick. Also logs a
//                          "boundary" line whenever the src changes. Low-volume
//                          and enough to characterise stutter vs. boundary
//                          stalls without drowning the console.
//   DEBUG_MEDIA_VERBOSE  — per-event stream (~20 events/sec during stutter on
//                          WebKitGTK). Turn on only for short captures when
//                          you need event ordering.
function mediaDebugEnabled(key: string): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

// Runtime toggles: set these in the WebView console and reload the app:
//   localStorage.setItem("tripviewer.debug.media", "1")
//   localStorage.setItem("tripviewer.debug.media.verbose", "1")
const DEBUG_MEDIA = mediaDebugEnabled("tripviewer.debug.media");
const DEBUG_MEDIA_VERBOSE = mediaDebugEnabled(
  "tripviewer.debug.media.verbose",
);

const MEDIA_EVENTS = [
  "loadstart",
  "loadedmetadata",
  "loadeddata",
  "canplay",
  "canplaythrough",
  "waiting",
  "stalled",
  "suspend",
  "emptied",
  "abort",
  "play",
  "playing",
  "pause",
  "seeking",
  "seeked",
  "ended",
  "error",
] as const;

import { probeVideoAt, recoveryDelayMs } from "./mediaRecovery";

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

interface Props {
  overlayCrop?: OverlayCrop;
  overlayFrame?: OverlayFrame;
  disablePausedZoom?: boolean;
  label: string;
  src: string;
  filePath: string;
  isMaster: boolean;
  /** Audio comes from the canonical SyncEngine master, not necessarily the
   *  visually enlarged camera. Keeping one stable audio clock prevents audio
   *  from jumping onto a slave pipeline that has just been re-seeked. */
  audioEnabled: boolean;
  onClick?: () => void;
  onDoubleClick?: () => void;
  onContextMenu?: (event: ReactMouseEvent<HTMLVideoElement>) => void;
  /** Report a terminal media failure for this source. The parent removes the
   *  failed channel from the active SyncEngine while keeping the panel mounted
   *  with its error overlay. A new source/segment gets a clean retry. */
  onMediaError?: (label: string, message: string, cached: boolean) => void;
  onMediaRecovered?: (label: string) => void;
}

export const ChannelPanel = forwardRef<HTMLVideoElement, Props>(
  function ChannelPanel({
    label,
    src,
    filePath,
    isMaster,
    audioEnabled,
    onClick,
    onDoubleClick,
    onContextMenu,
    onMediaError,
    onMediaRecovered,
    overlayCrop,
    overlayFrame,
    disablePausedZoom = false,
  }, ref) {
    const [error, setError] = useState<string | null>(null);
    const [ready, setReady] = useState(false);
    // True while this channel is in a coverage gap (camera was off for
    // this stretch in tiered playback). The SyncEngine holds the
    // `<video>`; we paint black over it. Always false in Original mode.
    const gapped = useStore((s) => s.gappedChannels[label] ?? false);
    const playbackSpeed = useStore((s) => s.speed);
    const sourceMode = useStore((s) => s.sourceMode);
    const audioMuted = shouldMuteChannelAudio(
      audioEnabled,
      playbackSpeed,
      sourceMode,
    );
    const audibleVolumeRef = useRef(1);
    // `showLoading` is `!ready` debounced by LOADING_OVERLAY_DELAY_MS.
    // Fast loads (the common case on Windows/Chromium and on macOS now
    // that the loopback HTTP server feeds AVFoundation moov immediately)
    // complete before this flips true, so the user sees a smooth cut
    // from one segment's last frame to the next segment's first frame
    // without a flash of "Loading…". Genuinely slow loads still show it.
    const [showLoading, setShowLoading] = useState(false);
    const [healthRanges, setHealthRanges] = useState<MediaHealthRange[]>([]);
    const localRef = useRef<HTMLVideoElement | null>(null);
    const failedRef = useRef(false);
    const probingRef = useRef(false);
    const sourceEpochRef = useRef(0);
    const failureRangeRef = useRef<MediaHealthRange | undefined>(undefined);

    // Merge the forwarded ref with our local ref so we can attach debug
    // listeners without disturbing whatever the parent is doing with the ref.
    const isPlaying = useStore(s => s.isPlaying);
    const canZoom = !overlayCrop && !disablePausedZoom && !isPlaying && ready && !error && !gapped;
    const zoom = usePausedZoom(localRef, canZoom, src);
    const cropStyle = useOverlayCrop(localRef, overlayCrop);

    const setRefs = useCallback(
      (node: HTMLVideoElement | null) => {
        localRef.current = node;
        if (typeof ref === "function") ref(node);
        else if (ref) ref.current = node;
      },
      [ref],
    );

    useEffect(() => {
      sourceEpochRef.current += 1;
      failedRef.current = false;
      probingRef.current = false;
      setError(null);
      setReady(false);
      setHealthRanges([]);
      const video = localRef.current;
      if (video) {
        video.pause();
        video.removeAttribute("src");
        video.load();
      }
      failureRangeRef.current = undefined;
      if (DEBUG_MEDIA) {
        console.log(`[media/${label}] boundary src=…${src.slice(-50)}`);
      }

      let cancelled = false;
      void loadMediaHealth([filePath])
        .then((ranges) => {
          if (!cancelled) {
            setHealthRanges(ranges);
            const t = useStore.getState().currentTime;
            const known = ranges.find((range) => t >= range.startS && (range.endS === null || t < range.endS));
            if (known) {
              failureRangeRef.current = known;
              failedRef.current = true;
              const message = `known media failure: ${known.errorKind}`;
              setError(message);
              onMediaError?.(label, message, true);
            }
            if (!known && localRef.current) localRef.current.src = src;
          }
        })
        .catch((healthError) => {
          console.warn(`[media/${label}] health cache load failed:`, healthError);
          if (!cancelled && localRef.current) localRef.current.src = src;
        });
      return () => {
        cancelled = true;
      };
    }, [src, filePath, label]);

    // WebKitGTK has shown that changing the React `muted` prop alone can
    // leave the underlying GStreamer audio branch audible across a playbackRate
    // transition. Mirror the policy imperatively onto the actual media element
    // as well. Volume=0 is a second independent guard; when audio is allowed
    // again we restore the last audible volume.
    useEffect(() => {
      const video = localRef.current;
      if (!video) return;

      if (audioMuted) {
        if (video.volume > 0) audibleVolumeRef.current = video.volume;
        video.muted = true;
        video.defaultMuted = true;
        video.volume = 0;
      } else {
        video.volume = audibleVolumeRef.current;
        video.defaultMuted = false;
        video.muted = false;
      }
    }, [audioMuted, src]);

    // The loading overlay covers the <video> until `loadeddata` fires,
    // so the user sees "Loading…" rather than the black <video> element
    // while the decoder is preparing the first frame.
    useEffect(() => {
      const video = localRef.current;
      if (!video) return;

      const onLoaded = () => {
        if (failedRef.current) return;
        setReady(true);
        setError(null);
      };
      video.addEventListener("loadeddata", onLoaded);
      if (video.readyState >= 2) setReady(true);

      return () => {
        video.removeEventListener("loadeddata", onLoaded);
      };
    }, [src]);

    const markFailed = useCallback(
      (
        displayMessage: string,
        failureTime: number,
        persist = true,
      ) => {
        if (failedRef.current) return;
        failedRef.current = true;
        localRef.current?.pause();
        setReady(false);
        setError(displayMessage);
        onMediaError?.(label, displayMessage, !persist);
        if (persist) {
          void recordMediaFailure(filePath, failureTime, displayMessage).catch(
            (healthError) => {
              console.warn(
                `[media/${label}] health cache failure write failed:`,
                healthError,
              );
            },
          );
        }
      },
      [filePath, label, onMediaError],
    );

    // If this exact file fingerprint has a known bad interval, isolate the
    // channel before WebKit spends CPU rediscovering the same decode failure.
    // Closed ranges recover at their recorded end; open ranges still get the
    // exponential probe loop below because their end is not known yet.
    useEffect(() => {
      if (healthRanges.length === 0) return;
      const handle = window.setInterval(() => {
        if (failedRef.current) return;
        const t = useStore.getState().currentTime;
        const known = healthRanges.find(
          (range) =>
            t >= range.startS &&
            (range.endS === null || t < range.endS),
        );
        if (!known) return;
        failureRangeRef.current = known;
        markFailed(
          `known media failure: ${known.errorKind}`,
          known.startS,
          false,
        );
      }, 250);
      return () => window.clearInterval(handle);
    }, [healthRanges, markFailed]);

    // Once isolated, probe only this failed camera while the healthy cameras
    // keep playing. Probe intervals grow 1s -> 2s -> 4s -> 8s -> 16s -> 30s.
    // A cached closed range suppresses probing until its known end, which saves
    // decoder work on repeat views.
    useEffect(() => {
      if (!error) return;
      const video = localRef.current;
      if (!video) return;

      const epoch = sourceEpochRef.current;
      let cancelled = false;
      const abort = new AbortController();
      // Keep repeated decoder resets off the visible failed pipeline. A tiny
      // rendered, muted element permits WebKit frame callbacks in background.
      const probe = document.createElement("video");
      probe.src = src;
      probe.muted = true;
      probe.playsInline = true;
      probe.style.cssText = "position:fixed;top:0;left:0;z-index:2147483647;width:1px;height:1px;opacity:0.01;pointer-events:none";
      probe.setAttribute("aria-hidden", "true");
      document.body.appendChild(probe);

      const run = async () => {
        let attempt = 0;
        while (!cancelled && failedRef.current && epoch === sourceEpochRef.current) {
          const nowT = useStore.getState().currentTime;
          const known = failureRangeRef.current;
          await delay(recoveryDelayMs(attempt, nowT, known));
          const currentT = useStore.getState().currentTime;
          if (known?.endS != null && currentT >= known.startS && currentT < known.endS) continue;
          if (
            cancelled ||
            !failedRef.current ||
            epoch !== sourceEpochRef.current
          ) {
            return;
          }

          // Also probe when every camera failed and transport is paused;
          // otherwise the final failure would deadlock recovery forever.
          const target = useStore.getState().currentTime;
          let ok = await probeVideoAt(probe, target, abort.signal);
          if (ok && !cancelled && epoch === sourceEpochRef.current) {
            probingRef.current = true;
            if (video.getAttribute("src") !== src) video.src = src;
            ok = await probeVideoAt(video, useStore.getState().currentTime, abort.signal);
            probingRef.current = false;
          }

          if (
            cancelled ||
            epoch !== sourceEpochRef.current ||
            !failedRef.current
          ) {
            return;
          }

          if (ok) {
            failedRef.current = false;
            setError(null);
            setReady(true);
            const recoveredAt = video.currentTime;
            setHealthRanges((ranges) => ranges.map((range) =>
              range.endS === null && range.startS <= recoveredAt
                ? { ...range, endS: recoveredAt } : range));
            failureRangeRef.current = undefined;
            void recordMediaRecovery(filePath, recoveredAt).catch((healthError) => {
              console.warn(
                `[media/${label}] health cache recovery write failed:`,
                healthError,
              );
            });
            onMediaRecovered?.(label);
            return;
          }

          // A previously closed range is only a hint: if decoding still fails
          // outside it, persist the newly observed failure rather than making
          // the next session rediscover the continuation again.
          if (known?.endS != null) {
            const continued: MediaHealthRange = {
              path: filePath, startS: target, endS: null, errorKind: known.errorKind,
            };
            failureRangeRef.current = continued;
            setHealthRanges((ranges) => [...ranges, continued]);
            void recordMediaFailure(filePath, target, known.errorKind).catch((healthError) => {
              console.warn(`[media/${label}] continued failure write failed:`, healthError);
            });
          }

          attempt += 1;
        }
      };

      void run();
      return () => {
        cancelled = true;
        abort.abort();
        probe.pause();
        probe.removeAttribute("src");
        probe.load();
        probe.remove();
        probingRef.current = false;
      };
    }, [error, filePath, src, label, onMediaRecovered]);

    // Debounce the overlay. If `ready` flips back to true within the
    // delay window (almost always, on a healthy machine), the timer is
    // cleared and the overlay never paints.
    useEffect(() => {
      if (ready) {
        setShowLoading(false);
        return;
      }
      const LOADING_OVERLAY_DELAY_MS = 150;
      const t = window.setTimeout(() => setShowLoading(true), LOADING_OVERLAY_DELAY_MS);
      return () => window.clearTimeout(t);
    }, [ready]);

    useEffect(() => {
      if (!DEBUG_MEDIA && !DEBUG_MEDIA_VERBOSE) return;
      const video = localRef.current;
      if (!video) return;

      const mountedAt = performance.now();
      const counts = new Map<string, number>();

      // Verbose per-event stream.
      const verboseHandler = (ev: Event) => {
        const dt = ((performance.now() - mountedAt) / 1000).toFixed(3);
        const v = ev.currentTarget as HTMLVideoElement;
        const tail = (v.currentSrc || v.src).slice(-50);
        const base =
          `[media/${label}] +${dt}s ${ev.type} ` +
          `rs=${v.readyState} ns=${v.networkState} ` +
          `t=${(v.currentTime).toFixed(3)} paused=${v.paused} ended=${v.ended} ` +
          `…${tail}`;
        if (ev.type === "error") {
          const err = v.error;
          console.error(`${base} code=${err?.code ?? "?"} msg=${err?.message ?? ""}`);
        } else {
          console.log(base);
        }
      };

      // Always-on counter (cheap).
      const countHandler = (ev: Event) => {
        counts.set(ev.type, (counts.get(ev.type) ?? 0) + 1);
      };

      for (const name of MEDIA_EVENTS) {
        if (DEBUG_MEDIA_VERBOSE) video.addEventListener(name, verboseHandler);
        if (DEBUG_MEDIA) video.addEventListener(name, countHandler);
      }

      // Summary tick every 1 s.
      let timer: number | null = null;
      if (DEBUG_MEDIA) {
        let lastWall = performance.now();
        let lastMedia = video.currentTime;
        timer = window.setInterval(() => {
          const nowWall = performance.now();
          const nowMedia = video.currentTime;
          const dWall = (nowWall - lastWall) / 1000;
          const dMedia = nowMedia - lastMedia;
          const rt = dWall > 0 ? ((dMedia / dWall) * 100).toFixed(0) : "—";
          const uptime = ((nowWall - mountedAt) / 1000).toFixed(1);

          // Buffered window end (how far ahead of currentTime is decoded data).
          let bufEnd = "—";
          let bufHead = "—";
          try {
            const b = video.buffered;
            if (b.length > 0) {
              const end = b.end(b.length - 1);
              bufEnd = end.toFixed(2);
              bufHead = (end - nowMedia).toFixed(2);
            }
          } catch {
            // some engines throw on buffered access with no data
          }

          const evSummary =
            counts.size === 0
              ? "events: (none)"
              : "events: " +
                Array.from(counts.entries())
                  .sort((a, b) => b[1] - a[1])
                  .map(([k, v]) => `${k}=${v}`)
                  .join(" ");

          console.log(
            `[media/${label}] +${uptime}s t=${nowMedia.toFixed(2)} ` +
              `Δt=${dMedia.toFixed(2)} rt=${rt}% ` +
              `rs=${video.readyState} ns=${video.networkState} ` +
              `buf=${bufEnd} ahead=${bufHead}s ${evSummary}`,
          );

          counts.clear();
          lastWall = nowWall;
          lastMedia = nowMedia;
        }, 1000);
      }

      return () => {
        if (timer !== null) window.clearInterval(timer);
        for (const name of MEDIA_EVENTS) {
          if (DEBUG_MEDIA_VERBOSE) video.removeEventListener(name, verboseHandler);
          if (DEBUG_MEDIA) video.removeEventListener(name, countHandler);
        }
      };
    }, [label]);

    return (
      <div
        style={overlayFrame ? {
          borderRadius: overlayFrame === "mirror" ? "18% 18% 24% 24% / 35% 35% 45% 45%" : "10px",
          border: overlayFrame === "mirror" ? "3px solid #71717a" : "2px solid #52525b",
          isolation: "isolate", transform: "translateZ(0)",
        } : undefined}
        title={canZoom ? "Paused: scroll to zoom, drag the enlarged image to pan" : undefined}
        onClickCapture={zoom.onClickCapture}
        onClick={onClick}
        onDoubleClick={onDoubleClick}
        className={clsx(
          "group relative h-full w-full overflow-hidden rounded-md bg-black",
          (onClick || onDoubleClick) && "cursor-pointer",
        )}
      >
        <video
          ref={setRefs}
          onPointerDown={zoom.onPointerDown}
          onPointerMove={zoom.onPointerMove}
          onPointerUp={zoom.onPointerUp}
          onPointerCancel={zoom.onPointerCancel}
          onLostPointerCapture={zoom.onLostPointerCapture}
          style={cropStyle ?? { transform: zoom.view.scale > 1 ? `translate(${zoom.view.x}px, ${zoom.view.y}px) scale(${zoom.view.scale})` : undefined,
            touchAction: canZoom && zoom.view.scale > 1 ? "none" : "auto",
            cursor: canZoom && zoom.view.scale > 1 ? "grab" : undefined }}
          className="h-full w-full object-contain"
          muted={audioMuted}
          preload="auto"
          playsInline
          onContextMenu={onContextMenu}
          onError={(e) => {
            const video = e.currentTarget as HTMLVideoElement;
            const code = video.error?.code ?? 0;
            const message = video.error?.message ?? "";
            const networkState = video.networkState;
            console.error(
              `[${label}] video error code=${code} networkState=${networkState} ` +
                `src=${video.currentSrc || video.src} message=${message}`,
            );

            const map: Record<number, string> = {
              1: "aborted",
              2: "network error (failed to load)",
              3: "decode error",
              4: "source not supported (load or codec failure)",
            };
            const displayMessage = message
              ? `${map[code] ?? `playback error ${code}`}: ${message}`
              : map[code] ?? `playback error ${code}`;

            // Probe failures are expected while a channel is isolated. The
            // recovery loop owns their backoff and must not repeatedly rebuild
            // the SyncEngine or create duplicate DB ranges.
            if (probingRef.current) return;

            markFailed(
              displayMessage,
              useStore.getState().currentTime,
              true,
            );
          }}
        />

        {canZoom && (
          <div className="absolute bottom-2 right-2 z-20 flex items-center gap-1 rounded bg-black/80 p-1 text-xs text-white"
            onClick={event => event.stopPropagation()} onDoubleClick={event => event.stopPropagation()}>
            <button type="button" aria-label={`Zoom out ${label}`} disabled={zoom.view.scale <= 1}
              className="rounded px-2 py-1 hover:bg-neutral-700 disabled:opacity-40" onClick={() => zoom.changeZoom(1 / 1.5)}>−</button>
            <button type="button" aria-label={`Reset zoom ${label}`} title="Reset zoom and pan"
              className="rounded px-2 py-1 hover:bg-neutral-700" onClick={zoom.reset}>{Math.round(zoom.view.scale * 100)}%</button>
            <button type="button" aria-label={`Zoom in ${label}`} disabled={zoom.view.scale >= 8}
              className="rounded px-2 py-1 hover:bg-neutral-700 disabled:opacity-40" onClick={() => zoom.changeZoom(1.5)}>+</button>
          </div>
        )}
        {gapped && (
          <div className="absolute inset-0 flex items-center justify-center bg-black text-xs text-neutral-600">
            camera off
          </div>
        )}

        <div hidden={overlayFrame === "mirror"} className="absolute left-2 top-2 flex flex-col items-start gap-1">
          <div
            className={clsx(
              "rounded px-2 py-1 text-xs font-medium uppercase tracking-wide backdrop-blur",
              isMaster ? "bg-blue-500/80 text-white" : "bg-black/60 text-neutral-200",
            )}
          >
            {label}
          </div>
          {onClick && (
            <div className="rounded bg-black/60 px-2 py-0.5 text-[10px] text-neutral-300 opacity-0 backdrop-blur transition-opacity group-hover:opacity-100">
              Click to enlarge
            </div>
          )}
          {onDoubleClick && (
            <div className="rounded bg-black/60 px-2 py-0.5 text-[10px] text-neutral-300 opacity-0 backdrop-blur transition-opacity group-hover:opacity-100">
              Double-click for fullscreen
            </div>
          )}
        </div>

        {showLoading && !error && (
          <div className="absolute inset-0 flex items-center justify-center bg-black text-xs text-neutral-500">
            Loading…
          </div>
        )}

        {error && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-1 bg-red-950/80 p-4 text-center">
            <div className="text-xs font-semibold uppercase tracking-wide text-red-300">
              {label}
            </div>
            <div className="text-xs text-red-200">{error}</div>
          </div>
        )}
      </div>
    );
  },
);
