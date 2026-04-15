import { CSSProperties, RefObject } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Channel, ChannelKind, Segment } from "../../types/model";
import { ChannelPanel } from "./ChannelPanel";
import { useStore } from "../../state/store";

// On Linux, Tauri's convertFileSrc returns `asset://localhost/...` which
// WebKitGTK's GStreamer-based media player can't load (no GStreamer URI
// handler for the `asset` scheme, so <video> fails with FormatError).
// `file://` URLs are blocked by cross-origin policy between the localhost
// webview and the filesystem. The workaround is a tiny HTTP server bound
// to 127.0.0.1 (see src-tauri/src/video_server.rs); we fetch its port via
// `get_video_port` at app startup. On Windows/macOS the default asset
// protocol is already HTTP-based and works fine, so videoPort is 0.
const IS_LINUX =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Linux") &&
  !navigator.userAgent.includes("Android");

function videoSrcFor(filePath: string, videoPort: number | null): string {
  if (IS_LINUX && videoPort && videoPort > 0) {
    // filePath is absolute on Linux (e.g. "/home/chris10/..."), so the
    // leading slash is already part of the URL path. Concatenating with
    // `/` in between would produce `http://host:port//home/...` — some
    // HTTP parsers treat that as empty authority + path, and our own
    // server used to reject it as non-absolute.
    return `http://127.0.0.1:${videoPort}${encodeURI(filePath)}`;
  }
  return convertFileSrc(filePath);
}

interface Props {
  frontRef: RefObject<HTMLVideoElement | null>;
  interiorRef: RefObject<HTMLVideoElement | null>;
  rearRef: RefObject<HTMLVideoElement | null>;
  activeSegment: Segment | null;
}

function channelByKind(segment: Segment, kind: ChannelKind): Channel | undefined {
  return segment.channels.find((c) => c.kind === kind);
}

function refForKind(
  kind: ChannelKind,
  frontRef: RefObject<HTMLVideoElement | null>,
  interiorRef: RefObject<HTMLVideoElement | null>,
  rearRef: RefObject<HTMLVideoElement | null>,
): RefObject<HTMLVideoElement | null> {
  switch (kind) {
    case "front":
      return frontRef;
    case "interior":
      return interiorRef;
    case "rear":
      return rearRef;
  }
}

/**
 * Compute CSS grid placement for a channel.
 * The primary channel spans the left column across both rows.
 * The two secondaries stack in the right column.
 */
function gridStyle(
  kind: ChannelKind,
  primaryChannel: ChannelKind,
  secondaryIndex: number,
): CSSProperties {
  if (kind === primaryChannel) {
    return { gridColumn: 1, gridRow: "1 / 3" };
  }
  return { gridColumn: 2, gridRow: secondaryIndex + 1 };
}

export function VideoGrid({
  frontRef,
  interiorRef,
  rearRef,
  activeSegment,
}: Props) {
  const primaryChannel = useStore((s) => s.primaryChannel);
  const setPrimaryChannel = useStore((s) => s.setPrimaryChannel);
  const videoPort = useStore((s) => s.videoPort);
  const multiChannelEnabled = useStore((s) => s.multiChannelEnabled);
  const setMultiChannelEnabled = useStore((s) => s.setMultiChannelEnabled);

  // On Linux, interior/rear channels are opt-in: three concurrent HEVC
  // pipelines can saturate memory bandwidth on low-VRAM iGPUs (Vega 11
  // observed) and in extreme cases hang the GPU. Windows/macOS are
  // unaffected and always show all three channels.
  const showSecondaries = !IS_LINUX || multiChannelEnabled;

  // On Linux, wait for the loopback video server port before rendering the
  // video elements. Rendering with an asset:// or file:// src first would
  // cause <video> to emit an error and poison playback.
  if (IS_LINUX && !videoPort) {
    return (
      <div className="col-span-2 flex items-center justify-center text-sm text-neutral-500">
        Starting video server…
      </div>
    );
  }

  if (!activeSegment) {
    return (
      <div className="col-span-2 flex items-center justify-center text-sm text-neutral-500">
        Select a trip from the list to begin playback.
      </div>
    );
  }

  const allKinds: ChannelKind[] = ["front", "interior", "rear"];
  const kindsToRender: ChannelKind[] = showSecondaries
    ? allKinds
    : [primaryChannel];
  // Track secondary index for grid row assignment
  let secondaryIdx = 0;

  function handleMainDoubleClick() {
    if (document.fullscreenElement) {
      document.exitFullscreen();
    } else {
      const ref = refForKind(primaryChannel, frontRef, interiorRef, rearRef);
      ref.current?.requestFullscreen();
    }
  }

  return (
    <div className="col-span-2 grid grid-cols-[2fr_1fr] grid-rows-2 gap-2">
      {kindsToRender.map((kind) => {
        const channel = channelByKind(activeSegment, kind);
        if (!channel) return null;

        const isPrimary = kind === primaryChannel;
        const idx = isPrimary ? 0 : secondaryIdx++;
        const ref = refForKind(kind, frontRef, interiorRef, rearRef);

        return (
          <div key={kind} style={gridStyle(kind, primaryChannel, idx)}>
            <ChannelPanel
              ref={ref}
              kind={kind}
              src={videoSrcFor(channel.filePath, videoPort)}
              isMaster={isPrimary}
              onClick={isPrimary ? undefined : () => setPrimaryChannel(kind)}
              onDoubleClick={isPrimary ? handleMainDoubleClick : undefined}
            />
          </div>
        );
      })}

      {!showSecondaries && (
        <button
          type="button"
          onClick={() => setMultiChannelEnabled(true)}
          style={{ gridColumn: 2, gridRow: "1 / 3" }}
          className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-md border border-dashed border-neutral-700 bg-neutral-900/50 p-4 text-center text-xs text-neutral-400 hover:border-neutral-500 hover:bg-neutral-900 hover:text-neutral-200"
        >
          <div className="font-semibold uppercase tracking-wide text-neutral-300">
            Interior &amp; Rear
          </div>
          <div>Click to enable multi-channel view</div>
          <div className="text-[10px] leading-snug text-neutral-500">
            Disabled by default on Linux. May cause stutter or, on low-VRAM
            iGPUs, hang the GPU. Press <kbd className="rounded bg-neutral-800 px-1">M</kbd> to toggle.
          </div>
        </button>
      )}
    </div>
  );
}
