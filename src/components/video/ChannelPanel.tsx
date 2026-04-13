import { forwardRef, useEffect, useState } from "react";
import clsx from "clsx";
import type { ChannelKind } from "../../types/model";

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

    useEffect(() => {
      setError(null);
      setReady(false);
    }, [src]);

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
          ref={ref}
          src={src}
          className="h-full w-full object-contain"
          muted={!isMaster}
          preload="auto"
          playsInline
          onLoadedData={() => setReady(true)}
          onError={(e) => {
            const code = (e.currentTarget as HTMLVideoElement).error?.code ?? 0;
            const map: Record<number, string> = {
              1: "aborted",
              2: "network error",
              3: "decode error",
              4: "unsupported codec (missing HEVC decoder?)",
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
