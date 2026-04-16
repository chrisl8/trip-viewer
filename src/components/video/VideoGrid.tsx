import { CSSProperties, MutableRefObject, useEffect } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Segment } from "../../types/model";
import { ChannelPanel } from "./ChannelPanel";
import { useStore } from "../../state/store";

// Both Linux and macOS need the tiny loopback HTTP server
// (src-tauri/src/video_server.rs) for <video> playback, for different
// reasons:
//
//   Linux (WebKitGTK + GStreamer): Tauri's convertFileSrc returns
//     `asset://localhost/...` but WebKitGTK's <video> has no URI handler
//     for the `asset` scheme and fails with FormatError. `file://` URLs
//     are blocked by cross-origin policy between the webview and the
//     filesystem.
//
//   macOS (WKWebView + AVFoundation): the asset:// handler on macOS does
//     not honor HTTP Range requests. Wolfbox MP4s have `moov` at EOF, so
//     without range support AVFoundation linearly buffers ~14 s of mdat
//     before it can start decoding the primary 4K channel.
//
// The Rust server is fully Range-capable (206 Partial Content), so
// whoever has a non-zero videoPort uses HTTP. Windows (WebView2) handles
// the default asset protocol correctly and gets videoPort = 0, falling
// through to convertFileSrc.
const IS_LINUX =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Linux") &&
  !navigator.userAgent.includes("Android");

const IS_MAC =
  typeof navigator !== "undefined" &&
  navigator.userAgent.includes("Mac OS X");

function videoSrcFor(filePath: string, videoPort: number | null): string {
  if (videoPort && videoPort > 0) {
    return `http://127.0.0.1:${videoPort}${encodeURI(filePath)}`;
  }
  return convertFileSrc(filePath);
}

interface Props {
  /** Shared map of label → <video> element, populated by callback refs.
   *  Stable identity across renders so useSyncEngine doesn't re-run. */
  channelRefs: MutableRefObject<Map<string, HTMLVideoElement | null>>;
  activeSegment: Segment | null;
}

/**
 * Compute CSS grid placement for a channel panel.
 *
 * Layout philosophy: primary takes col 1 full height; secondaries stack
 * in col 2. Row count adapts to secondary count so each secondary gets
 * the full column width. Works for 1, 2, 3, 4+ channels.
 */
function gridStyle(
  isPrimary: boolean,
  secondaryIndex: number,
  secondaryCount: number,
  rowCount: number,
): CSSProperties {
  if (secondaryCount === 0) {
    // Single channel: fill the whole area.
    return { gridColumn: "1 / 3", gridRow: "1 / 3" };
  }
  if (isPrimary) {
    // Primary occupies col 1, spanning all rows. Using rowCount (not
    // secondaryCount) ensures the primary fills the full grid height even
    // when rowCount > secondaryCount (the 2-channel case, where rowCount=2
    // but secondaryCount=1). Without this, an empty grid row captures
    // pointer events from the transport controls below via a Chromium
    // compositor hit-testing edge case.
    return { gridColumn: 1, gridRow: `1 / ${rowCount + 1}` };
  }
  // Secondary cell: col 2, one row per secondary.
  return { gridColumn: 2, gridRow: secondaryIndex + 1 };
}

export function VideoGrid({ channelRefs, activeSegment }: Props) {
  const primaryChannel = useStore((s) => s.primaryChannel);
  const setPrimaryChannel = useStore((s) => s.setPrimaryChannel);
  const videoPort = useStore((s) => s.videoPort);
  const multiChannelEnabled = useStore((s) => s.multiChannelEnabled);
  const setMultiChannelEnabled = useStore((s) => s.setMultiChannelEnabled);

  // On first render of a segment (or when primaryChannel is null from a
  // trip/segment change), initialize primary to the first channel in
  // canonical order. This is also the sync master.
  useEffect(() => {
    if (!activeSegment) return;
    const master = activeSegment.channels[0]?.label ?? null;
    if (!master) return;
    // If primaryChannel is stale (references a label no longer in the
    // segment, e.g. after switching from 3-channel Wolf Box to 2-channel
    // Thinkware), reset it.
    const valid = activeSegment.channels.some((c) => c.label === primaryChannel);
    if (!valid) setPrimaryChannel(master);
  }, [activeSegment, primaryChannel, setPrimaryChannel]);

  // On Linux, additional channels are opt-in: three or more concurrent
  // HEVC pipelines can saturate memory bandwidth on low-VRAM iGPUs.
  // Windows/macOS are unaffected and always render everything.
  const showSecondaries = !IS_LINUX || multiChannelEnabled;

  if ((IS_LINUX || IS_MAC) && !videoPort) {
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

  // Always use canonical (Rust-sorted) order. The store's `primaryChannel`
  // label just tells us which of the rendered panels gets the primary
  // slot — it doesn't change tree order.
  const channels = activeSegment.channels;
  const effectivePrimary =
    channels.find((c) => c.label === primaryChannel)?.label ??
    channels[0]?.label;

  // On Linux in single-channel mode, we render ONLY the primary.
  // Otherwise we render all channels.
  const toRender = showSecondaries
    ? channels
    : channels.filter((c) => c.label === effectivePrimary);

  const secondaries = toRender.filter((c) => c.label !== effectivePrimary);

  function setRef(label: string) {
    return (node: HTMLVideoElement | null) => {
      if (node) {
        channelRefs.current.set(label, node);
      } else {
        channelRefs.current.delete(label);
      }
    };
  }

  function handleMainDoubleClick() {
    if (document.fullscreenElement) {
      document.exitFullscreen();
      return;
    }
    const el = channelRefs.current.get(effectivePrimary);
    el?.requestFullscreen();
  }

  // Row template: if primary takes full height and there are N
  // secondaries, we need N rows. Minimum of 2 rows for aesthetic
  // symmetry when there's only 1 secondary.
  const rowCount = Math.max(secondaries.length, 2);
  const gridTemplateRows = `repeat(${rowCount}, minmax(0, 1fr))`;

  return (
    <div
      className="col-span-2 grid grid-cols-[2fr_1fr] gap-2"
      style={{ gridTemplateRows }}
    >
      {toRender.map((channel) => {
        const isPrimary = channel.label === effectivePrimary;
        const idx = isPrimary
          ? 0
          : secondaries.findIndex((c) => c.label === channel.label);

        return (
          <div
            key={channel.label}
            style={gridStyle(isPrimary, idx, secondaries.length, rowCount)}
          >
            <ChannelPanel
              ref={setRef(channel.label)}
              label={channel.label}
              src={videoSrcFor(channel.filePath, videoPort)}
              isMaster={isPrimary}
              onClick={isPrimary ? undefined : () => setPrimaryChannel(channel.label)}
              onDoubleClick={isPrimary ? handleMainDoubleClick : undefined}
            />
          </div>
        );
      })}

      {!showSecondaries && channels.length > 1 && (
        <button
          type="button"
          onClick={() => setMultiChannelEnabled(true)}
          style={{ gridColumn: 2, gridRow: `1 / ${rowCount + 1}` }}
          className="flex h-full w-full flex-col items-center justify-center gap-2 rounded-md border border-dashed border-neutral-700 bg-neutral-900/50 p-4 text-center text-xs text-neutral-400 hover:border-neutral-500 hover:bg-neutral-900 hover:text-neutral-200"
        >
          <div className="font-semibold uppercase tracking-wide text-neutral-300">
            Other channels
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
