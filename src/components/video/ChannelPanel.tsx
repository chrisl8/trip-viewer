import { forwardRef, useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import type { ChannelKind } from "../../types/model";

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
const DEBUG_MEDIA = false;
const DEBUG_MEDIA_VERBOSE = false;

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

interface Props {
  kind: ChannelKind;
  src: string;
  isMaster: boolean;
  onClick?: () => void;
  onDoubleClick?: () => void;
}

export const ChannelPanel = forwardRef<HTMLVideoElement, Props>(
  function ChannelPanel({ kind, src, isMaster, onClick, onDoubleClick }, ref) {
    const [error, setError] = useState<string | null>(null);
    const [ready, setReady] = useState(false);
    const localRef = useRef<HTMLVideoElement | null>(null);

    // Merge the forwarded ref with our local ref so we can attach debug
    // listeners without disturbing whatever the parent is doing with the ref.
    const setRefs = useCallback(
      (node: HTMLVideoElement | null) => {
        localRef.current = node;
        if (typeof ref === "function") ref(node);
        else if (ref) ref.current = node;
      },
      [ref],
    );

    useEffect(() => {
      setError(null);
      setReady(false);
      if (DEBUG_MEDIA) {
        console.log(`[media/${kind}] boundary src=…${src.slice(-50)}`);
      }
    }, [src, kind]);

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
          `[media/${kind}] +${dt}s ${ev.type} ` +
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
            `[media/${kind}] +${uptime}s t=${nowMedia.toFixed(2)} ` +
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
    }, [kind]);

    return (
      <div
        onClick={onClick}
        onDoubleClick={onDoubleClick}
        className={clsx(
          "group relative h-full w-full overflow-hidden rounded-md bg-black",
          (onClick || onDoubleClick) && "cursor-pointer",
        )}
      >
        <video
          ref={setRefs}
          src={src}
          className="h-full w-full object-contain"
          muted={!isMaster}
          preload="auto"
          playsInline
          onLoadedData={() => setReady(true)}
          onError={(e) => {
            const video = e.currentTarget as HTMLVideoElement;
            const code = video.error?.code ?? 0;
            const message = video.error?.message ?? "";
            const networkState = video.networkState;
            // Dump everything the browser knows so the terminal/devtools
            // console can disambiguate "couldn't load" from "couldn't decode".
            console.error(
              `[${kind}] video error code=${code} networkState=${networkState} ` +
                `src=${video.currentSrc || video.src} message=${message}`,
            );
            const map: Record<number, string> = {
              1: "aborted",
              2: "network error (failed to load)",
              3: "decode error",
              4: "source not supported (load or codec failure)",
            };
            setError(map[code] ?? `playback error ${code}`);
          }}
        />

        <div className="absolute left-2 top-2 flex flex-col items-start gap-1">
          <div
            className={clsx(
              "rounded px-2 py-1 text-xs font-medium uppercase tracking-wide backdrop-blur",
              isMaster ? "bg-blue-500/80 text-white" : "bg-black/60 text-neutral-200",
            )}
          >
            {kind}
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

        {!ready && !error && (
          <div className="absolute inset-0 flex items-center justify-center text-xs text-neutral-500">
            Loading…
          </div>
        )}

        {error && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-1 bg-red-950/80 p-4 text-center">
            <div className="text-xs font-semibold uppercase tracking-wide text-red-300">
              {kind}
            </div>
            <div className="text-xs text-red-200">{error}</div>
          </div>
        )}
      </div>
    );
  },
);
