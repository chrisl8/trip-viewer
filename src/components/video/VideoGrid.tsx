import {
  CSSProperties,
  MutableRefObject,
  PointerEvent as ReactPointerEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import type { Segment } from "../../types/model";
import { channelGridStyle as gridStyle } from "./channelLayout";
import { ChannelPanel } from "./ChannelPanel";
import { OverlayHandles } from "./OverlayHandles";
import type { CameraLayoutController } from "./useCameraLayout";
import { overlayBackground, overlayPanelFor } from "../../settings/overlayLayout";
import { useStore } from "../../state/store";
import { videoSrcFor } from "../../utils/videoSrc";
import {
  getLayoutPreferences,
  saveLayoutPreferences,
} from "../../settings/layout";
import {
  currentFullscreenView,
  exitFullscreenView,
  openFullscreenView,
  type FullscreenStack,
  type FullscreenView,
} from "./fullscreenState";

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
//   macOS (WKWebView + AVFoundation): the asset:// handler on macOS
//     does not honor HTTP Range requests. Wolfbox MP4s have `moov` at EOF,
//     so without range support AVFoundation linearly buffers ~14 s of mdat
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

interface Props {
  cameraLayout: CameraLayoutController;
  hiddenChannels: string[];
  onFullscreenKindChange: (kind: "dashboard" | "video" | null) => void;
  /** Shared map of label → <video> element, populated by callback refs.
   *  Stable identity across renders so useSyncEngine doesn't re-run. */
  channelRefs: MutableRefObject<Map<string, HTMLVideoElement | null>>;
  activeSegment: Segment | null;
  /** Current SyncEngine clock/master label. May differ from the canonical
   *  front channel after a per-camera decode failure. */
  syncMasterLabel: string | null;
  onChannelMediaError?: (
    label: string,
    message: string,
    cached: boolean,
  ) => void;
  onChannelMediaRecovered?: (label: string) => void;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function VideoGrid({
  cameraLayout,
  hiddenChannels,
  onFullscreenKindChange,
  channelRefs,
  activeSegment,
  syncMasterLabel,
  onChannelMediaError,
  onChannelMediaRecovered,
}: Props) {
  const primaryChannel = useStore((s) => s.primaryChannel);
  const setPrimaryChannel = useStore((s) => s.setPrimaryChannel);
  const videoPort = useStore((s) => s.videoPort);
  const isPlaying = useStore((s) => s.isPlaying);
  const gridRef = useRef<HTMLDivElement | null>(null);
  const contextMenuRef = useRef<HTMLDivElement | null>(null);
  const initialLayout = useRef(getLayoutPreferences()).current;
  const [fullscreenStack, setFullscreenStack] = useState<FullscreenStack>([]);
  const fullscreenStackRef = useRef<FullscreenStack>([]);
  const fullscreenView = currentFullscreenView(fullscreenStack);
  const dashboardFullscreen = fullscreenView?.kind === "dashboard";
  const singleFullscreenLabel =
    fullscreenView?.kind === "video" ? fullscreenView.label : null;
  const fullscreenActive = fullscreenView !== null;
  useEffect(() => { onFullscreenKindChange(fullscreenView?.kind ?? null); }, [fullscreenView?.kind, onFullscreenKindChange]);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    videoLabel: string | null;
  } | null>(null);

  const [primaryShare, setPrimaryShare] = useState(initialLayout.primaryShare);
  const [secondarySplit, setSecondarySplit] = useState(initialLayout.secondarySplit);
  const [mapShare, setMapShare] = useState(initialLayout.mapShare);
  const [timelineHeightPx, setTimelineHeightPx] = useState<number | null>(
    initialLayout.timelineHeightPx,
  );
  const [hasMapPanel, setHasMapPanel] = useState(false);

  // Persist settled drag positions without synchronously writing localStorage
  // on every pointermove. The short debounce resets during a drag and normally
  // writes only once after the user releases the handle.
  useEffect(() => {
    const handle = window.setTimeout(() => {
      saveLayoutPreferences({
        primaryShare,
        secondarySplit,
        mapShare,
        timelineHeightPx,
      });
    }, 150);
    return () => window.clearTimeout(handle);
  }, [primaryShare, secondarySplit, mapShare, timelineHeightPx]);

  // On first render of a segment (or when primaryChannel is null from a
  // trip/segment change), initialize the visual primary to the first channel
  // in canonical order. The sync engine keeps its own stable canonical master;
  // changing the visual primary does not rebuild or retarget that engine.
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

  const visibleChannels = activeSegment?.channels.filter(c => !hiddenChannels.includes(c.label)) ?? [];
  const allChannelsHidden = Boolean(activeSegment) && visibleChannels.length === 0;

  // The outer PlayerShell grid is the *only* browser Fullscreen API element.
  // Logical dashboard/video fullscreen modes merely change what is rendered
  // inside it. This avoids browser-dependent nested fullscreen stacks while
  // preserving a deterministic back-stack in our own state machine.
  useEffect(() => {
    const grid = gridRef.current;
    const viewingArea = grid?.parentElement;
    if (!grid || !viewingArea) return;

    const sibling = grid.nextElementSibling as HTMLElement | null;
    const mapPresent = Boolean(sibling?.querySelector(".leaflet-container"));
    setHasMapPanel(mapPresent);

    // Restore any inline state left by the previous logical fullscreen mode.
    grid.style.gridColumn = "";
    if (mapPresent && sibling) sibling.style.display = "";

    if (singleFullscreenLabel) {
      grid.style.gridColumn = "1 / -1";
      viewingArea.style.gridTemplateColumns = "minmax(0, 1fr)";
      if (mapPresent && sibling) sibling.style.display = "none";
    } else if (allChannelsHidden && mapPresent && sibling) {
      grid.style.display = "none";
      sibling.style.gridColumn = "1 / -1";
      viewingArea.style.gridTemplateColumns = "minmax(0, 1fr)";
    } else if (!mapPresent) {
      viewingArea.style.gridTemplateColumns = "";
    } else {
      viewingArea.style.gridTemplateColumns =
        `minmax(0, ${1 - mapShare}fr) 0px minmax(180px, ${mapShare}fr)`;
    }

    return () => {
      grid.style.gridColumn = "";
      grid.style.display = "";
      if (sibling) sibling.style.gridColumn = "";
      viewingArea.style.gridTemplateColumns = "";
      if (mapPresent && sibling) sibling.style.display = "";
    };
  }, [activeSegment, mapShare, singleFullscreenLabel, allChannelsHidden]);

  // Timeline height is inherited as a CSS custom property. This keeps the
  // lower strip structurally identical and lets the flexing video area give
  // up or reclaim space naturally when the user drags the horizontal handle.
  useEffect(() => {
    const shell = gridRef.current?.parentElement?.parentElement;
    if (!shell) return;
    if (timelineHeightPx === null) {
      shell.style.removeProperty("--tripviewer-timeline-height");
    } else {
      shell.style.setProperty(
        "--tripviewer-timeline-height",
        `${timelineHeightPx}px`,
      );
    }
  }, [timelineHeightPx]);

  // Browser Escape (or any external fullscreen exit) clears the whole logical
  // stack. Normal in-app transitions never nest Fullscreen API elements; they
  // only push/pop logical views while the same viewing-area element remains
  // fullscreen.
  useEffect(() => {
    const onFullscreenChange = () => {
      const viewingArea = gridRef.current?.closest<HTMLElement>("[data-tripviewer-player]") ?? null;
      if (viewingArea && document.fullscreenElement === viewingArea) return;
      if (fullscreenStackRef.current.length === 0) return;

      fullscreenStackRef.current = [];
      setFullscreenStack([]);
      setContextMenu(null);
      resyncPlayback();
    };

    document.addEventListener("fullscreenchange", onFullscreenChange);
    return () => document.removeEventListener("fullscreenchange", onFullscreenChange);
  }, []);

  // Replace the generic webview menu over the viewing area with the small set
  // of actions that is useful during dashcam playback. Right-clicking a video
  // records that channel so the menu can offer context-aware single-camera
  // fullscreen in addition to dashboard fullscreen.
  useEffect(() => {
    const viewingArea = gridRef.current?.parentElement;
    if (!viewingArea || !activeSegment) return;

    const onContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      const target =
        event.target instanceof Element
          ? (event.target.closest("[data-tripviewer-channel]") as HTMLElement | null)
          : null;
      setContextMenu({
        x: event.clientX,
        y: event.clientY,
        videoLabel: target?.dataset.tripviewerChannel ?? null,
      });
    };

    viewingArea.addEventListener("contextmenu", onContextMenu);
    return () => {
      viewingArea.removeEventListener("contextmenu", onContextMenu);
      setContextMenu(null);
    };
  }, [activeSegment]);

  useEffect(() => {
    if (!contextMenu) return;

    const onPointerDown = (event: PointerEvent) => {
      if (
        contextMenuRef.current &&
        event.target instanceof Node &&
        contextMenuRef.current.contains(event.target)
      ) {
        return;
      }
      setContextMenu(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setContextMenu(null);
    };

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("blur", () => setContextMenu(null), { once: true });
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [contextMenu]);

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
  const canonicalMasterLabel = channels[0]?.label ?? null;
  const effectivePrimary =
    visibleChannels.find((c) => c.label === primaryChannel)?.label ??
    visibleChannels[0]?.label ?? null;

  const secondaries = visibleChannels.filter((c) => c.label !== effectivePrimary);
  const overlayMode = cameraLayout.mode === "overlay" && singleFullscreenLabel === null;
  const backgroundLabel = overlayBackground(visibleChannels.map(channel => channel.label));
  const overlayLabels = visibleChannels.filter(channel => channel.label !== backgroundLabel).map(channel => channel.label);
  const selectedOverlay = overlayLabels.find(label => label === cameraLayout.selected) ?? overlayLabels[0];

  function setRef(label: string) {
    return (node: HTMLVideoElement | null) => {
      if (node) {
        channelRefs.current.set(label, node);
      } else {
        channelRefs.current.delete(label);
      }
    };
  }

  function togglePlayback() {
    // TransportControls owns the SyncEngine instance. Dispatching this app
    // event lets fullscreen UI use the exact same pause/play path as the
    // ordinary transport bar and keyboard shortcut.
    window.dispatchEvent(new Event("tripviewer:toggle-playback"));
  }

  function resyncPlayback() {
    // Keep playback ownership inside SyncEngine. Visual viewer actions can
    // request a one-shot re-anchor without reaching into individual videos.
    window.dispatchEvent(new Event("tripviewer:resync-playback"));
  }

  function selectPrimary(label: string) {
    setPrimaryChannel(label);
    resyncPlayback();
  }

  function startPointerDrag(
    event: ReactPointerEvent<HTMLDivElement>,
    cursor: "col-resize" | "row-resize",
    onMove: (event: PointerEvent) => void,
  ) {
    event.preventDefault();
    event.stopPropagation();
    const oldUserSelect = document.body.style.userSelect;
    const oldCursor = document.body.style.cursor;
    document.body.style.userSelect = "none";
    document.body.style.cursor = cursor;

    const move = (pointerEvent: PointerEvent) => {
      pointerEvent.preventDefault();
      onMove(pointerEvent);
    };
    const finish = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      document.body.style.userSelect = oldUserSelect;
      document.body.style.cursor = oldCursor;
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  }

  function resizePrimarySecondary(event: ReactPointerEvent<HTMLDivElement>) {
    const rect = gridRef.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return;
    startPointerDrag(event, "col-resize", (pointerEvent) => {
      const share = (pointerEvent.clientX - rect.left) / rect.width;
      setPrimaryShare(clamp(share, 0.35, 0.85));
    });
  }

  function resizeSecondaryStack(event: ReactPointerEvent<HTMLDivElement>) {
    const rect = gridRef.current?.getBoundingClientRect();
    if (!rect || rect.height <= 0) return;
    startPointerDrag(event, "row-resize", (pointerEvent) => {
      const share = (pointerEvent.clientY - rect.top) / rect.height;
      setSecondarySplit(clamp(share, 0.2, 0.8));
    });
  }

  function resizeMap(event: ReactPointerEvent<HTMLDivElement>) {
    const viewingArea = gridRef.current?.parentElement;
    const rect = viewingArea?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return;
    startPointerDrag(event, "col-resize", (pointerEvent) => {
      const share = (rect.right - pointerEvent.clientX) / rect.width;
      setMapShare(clamp(share, 0.15, 0.5));
    });
  }

  function resizeTimeline(event: ReactPointerEvent<HTMLDivElement>) {
    const shell = gridRef.current?.parentElement?.parentElement;
    const timeline = shell?.querySelector(
      "[data-tripviewer-timeline]",
    ) as HTMLElement | null;
    if (!shell || !timeline) return;
    const startY = event.clientY;
    const startHeight = timeline.getBoundingClientRect().height;
    const maxHeight = Math.max(96, shell.getBoundingClientRect().height * 0.45);
    startPointerDrag(event, "row-resize", (pointerEvent) => {
      const next = startHeight + startY - pointerEvent.clientY;
      setTimelineHeightPx(clamp(next, 56, maxHeight));
    });
  }

  function commitFullscreenStack(next: FullscreenStack) {
    fullscreenStackRef.current = next;
    setFullscreenStack(next);
  }

  async function openLogicalFullscreen(target: FullscreenView) {
    const viewingArea = gridRef.current?.closest<HTMLElement>("[data-tripviewer-player]");
    if (!viewingArea) return;

    // Self-heal if browser fullscreen was lost before its event reached us.
    const browserHasViewer = document.fullscreenElement === viewingArea;
    const currentStack = browserHasViewer ? fullscreenStackRef.current : [];
    const next = openFullscreenView(currentStack, target);
    if (next === currentStack) return;

    commitFullscreenStack(next);
    if (!browserHasViewer) {
      try {
        await viewingArea.requestFullscreen();
      } catch (error) {
        commitFullscreenStack(currentStack);
        console.warn("[viewer] fullscreen request failed", error);
        return;
      }
    }
    resyncPlayback();
  }

  async function exitLogicalFullscreen() {
    const viewingArea = gridRef.current?.closest<HTMLElement>("[data-tripviewer-player]");
    if (!viewingArea) return;

    const currentStack = fullscreenStackRef.current;
    if (currentStack.length === 0) return;
    const next = exitFullscreenView(currentStack);

    if (next.length > 0) {
      commitFullscreenStack(next);
      resyncPlayback();
      return;
    }

    if (document.fullscreenElement === viewingArea) {
      try {
        await document.exitFullscreen();
      } catch (error) {
        console.warn("[viewer] fullscreen exit failed", error);
      }
      return;
    }

    commitFullscreenStack([]);
    resyncPlayback();
  }

  async function toggleSingleVideoFullscreen(label: string) {
    const current = currentFullscreenView(fullscreenStackRef.current);
    if (current?.kind === "video") {
      await exitLogicalFullscreen();
      return;
    }
    await openLogicalFullscreen({ kind: "video", label });
  }

  async function handleMainDoubleClick() {
    if (effectivePrimary) await toggleSingleVideoFullscreen(effectivePrimary);
  }

  async function toggleDashboardFullscreen() {
    const current = currentFullscreenView(fullscreenStackRef.current);
    if (current?.kind === "dashboard") {
      await exitLogicalFullscreen();
      return;
    }
    await openLogicalFullscreen({ kind: "dashboard" });
  }

  // Row template: if primary takes full height and there are N
  // secondaries, we need N rows. Minimum of 2 rows for aesthetic
  // symmetry when there's only 1 secondary.
  const rowCount = Math.max(secondaries.length, 2);
  const gridTemplateRows =
    secondaries.length === 2
      ? `${secondarySplit}fr ${1 - secondarySplit}fr`
      : `repeat(${rowCount}, minmax(0, 1fr))`;

  return (
    <div
      ref={gridRef}
      className={`relative col-span-2 min-h-0 min-w-0 bg-black ${overlayMode ? "isolate" : "grid gap-2"}`}
      style={{
        gridTemplateColumns: singleFullscreenLabel
          ? "minmax(0, 1fr)"
          : `${primaryShare}fr ${1 - primaryShare}fr`,
        gridTemplateRows: singleFullscreenLabel ? "minmax(0, 1fr)" : gridTemplateRows,
      }}
    >
      <div className="absolute right-2 top-2 z-[60] flex items-center gap-2">
        {singleFullscreenLabel !== null && (
          <button
            type="button"
            onClick={togglePlayback}
            className="rounded bg-blue-600/90 px-3 py-1.5 text-xs font-medium text-white backdrop-blur transition-colors hover:bg-blue-500"
            title={isPlaying ? "Pause playback" : "Resume playback"}
          >
            {isPlaying ? "Pause" : "Play"}
          </button>
        )}
        <button
          type="button"
          onClick={() => void toggleDashboardFullscreen()}
          className="rounded bg-black/70 px-2.5 py-1.5 text-xs font-medium text-neutral-100 opacity-70 backdrop-blur transition-opacity hover:opacity-100"
          title={
            dashboardFullscreen
              ? "Exit F/I/R + GPS fullscreen"
              : "Fullscreen F/I/R + GPS"
          }
        >
          {dashboardFullscreen ? "Exit dashboard" : "F/I/R + GPS fullscreen"}
        </button>
      </div>

      {!overlayMode && !singleFullscreenLabel && secondaries.length > 0 && (
        <div
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize main and secondary cameras"
          onPointerDown={resizePrimarySecondary}
          className="absolute bottom-0 top-0 z-30 w-3 -translate-x-1/2 cursor-col-resize"
          style={{ left: `${primaryShare * 100}%` }}
        >
          <div className="mx-auto h-full w-px bg-neutral-500/0 transition-colors hover:bg-neutral-400/70" />
        </div>
      )}

      {!overlayMode && !singleFullscreenLabel && secondaries.length === 2 && (
        <div
          role="separator"
          aria-orientation="horizontal"
          title="Drag to resize the secondary cameras"
          onPointerDown={resizeSecondaryStack}
          className="absolute right-0 z-30 h-3 -translate-y-1/2 cursor-row-resize"
          style={{
            left: `${primaryShare * 100}%`,
            top: `${secondarySplit * 100}%`,
          }}
        >
          <div className="my-auto h-px w-full bg-neutral-500/0 transition-colors hover:bg-neutral-400/70" />
        </div>
      )}

      {!singleFullscreenLabel && hasMapPanel && (
        <div
          role="separator"
          aria-orientation="vertical"
          title="Drag to resize video and GPS map"
          onPointerDown={resizeMap}
          className="absolute -right-2 bottom-0 top-0 z-30 w-4 cursor-col-resize"
        >
          <div className="mx-auto h-full w-px bg-neutral-500/0 transition-colors hover:bg-neutral-400/70" />
        </div>
      )}

      {!fullscreenActive && (
        <div
          role="separator"
          aria-orientation="horizontal"
          title="Drag to resize timeline and playback controls"
          onPointerDown={resizeTimeline}
          className="absolute -bottom-2 left-0 right-0 z-30 h-4 cursor-row-resize"
        >
          <div className="my-auto h-px w-full bg-neutral-500/0 transition-colors hover:bg-neutral-400/70" />
        </div>
      )}

      {contextMenu && (
        <div
          ref={contextMenuRef}
          className="fixed z-[70] min-w-[13rem] overflow-hidden rounded-md border border-neutral-700 bg-neutral-900 py-1 shadow-xl"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          role="menu"
        >
          <button
            type="button"
            onClick={() => {
              setContextMenu(null);
              togglePlayback();
            }}
            className="block w-full px-3 py-2 text-left text-sm text-neutral-100 hover:bg-neutral-800"
            role="menuitem"
          >
            {isPlaying ? "Pause" : "Play"}
          </button>
          {contextMenu.videoLabel && (
            <button
              type="button"
              onClick={() => {
                const label = contextMenu.videoLabel;
                setContextMenu(null);
                if (label) void toggleSingleVideoFullscreen(label);
              }}
              className="block w-full px-3 py-2 text-left text-sm text-neutral-100 hover:bg-neutral-800"
              role="menuitem"
            >
              {fullscreenView?.kind === "video"
                ? "Exit video fullscreen"
                : "Open video fullscreen"}
            </button>
          )}
          <button
            type="button"
            onClick={() => {
              setContextMenu(null);
              void toggleDashboardFullscreen();
            }}
            className="block w-full px-3 py-2 text-left text-sm text-neutral-100 hover:bg-neutral-800"
            role="menuitem"
          >
            {dashboardFullscreen
              ? "Exit F/I/R + GPS fullscreen"
              : "F/I/R + GPS fullscreen"}
          </button>
          <div className="my-1 border-t border-neutral-700" />
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="block w-full px-3 py-2 text-left text-sm text-neutral-300 hover:bg-neutral-800 hover:text-white"
            role="menuitem"
          >
            Reload
          </button>
        </div>
      )}

      {allChannelsHidden && !hasMapPanel && <div className={`${overlayMode ? "absolute inset-0" : "col-span-full row-span-full"} flex items-center justify-center text-sm text-neutral-400`}>All cameras hidden. Select a camera in Settings to show video.</div>}
      {channels.map((channel) => {
        const isPrimary = channel.label === effectivePrimary;
        const idx = isPrimary
          ? 0
          : secondaries.findIndex((c) => c.label === channel.label);

        const singleTarget = singleFullscreenLabel === channel.label;
        const singleHidden = hiddenChannels.includes(channel.label) ||
          (singleFullscreenLabel !== null && !singleTarget);
        const overlayIndex = overlayLabels.indexOf(channel.label);
        const overlayPanel = overlayMode && channel.label !== backgroundLabel
          ? overlayPanelFor(cameraLayout.panels, channel.label, Math.max(0, overlayIndex)) : undefined;
        const style: CSSProperties = hiddenChannels.includes(channel.label) ? { display: "none" } : singleFullscreenLabel
          ? singleTarget
            ? { gridColumn: "1 / -1", gridRow: "1 / -1" }
            : { display: "none" }
          : overlayMode
            ? overlayPanel
              ? { position: "absolute", left: `${overlayPanel.x * 100}%`, top: `${overlayPanel.y * 100}%`,
                  width: `${overlayPanel.width * 100}%`, height: `${overlayPanel.height * 100}%`, zIndex: overlayPanel.order }
              : { position: "absolute", inset: 0, zIndex: 0 }
            : gridStyle(isPrimary, idx, secondaries.length, rowCount);

        return (
          <div
            key={channel.label}
            data-tripviewer-channel={channel.label}
            data-overlay-role={overlayMode ? overlayPanel ? "overlay" : "background" : undefined}
            style={style}
            aria-hidden={singleHidden || undefined}
          >
            <ChannelPanel
              ref={setRef(channel.label)}
              label={channel.label}
              src={videoSrcFor(channel.filePath, videoPort)}
              filePath={channel.filePath}
              isMaster={isPrimary}
              audioEnabled={channel.label === (syncMasterLabel ?? canonicalMasterLabel)}
              onMediaError={onChannelMediaError}
              onMediaRecovered={onChannelMediaRecovered}
              overlayCrop={overlayPanel?.crop}
              overlayFrame={overlayPanel?.frame}
              disablePausedZoom={overlayMode && cameraLayout.editing}
              onClick={overlayMode || isPrimary ? undefined : () => selectPrimary(channel.label)}
              onDoubleClick={overlayMode ? () => { void toggleSingleVideoFullscreen(channel.label); } : isPrimary ? handleMainDoubleClick : undefined}
              onContextMenu={(event) => {
                // WebKitGTK otherwise shows its own media menu with a static
                // "Switch to Fullscreen" item even while already fullscreen.
                event.preventDefault();
                event.stopPropagation();
                setContextMenu({
                  x: event.clientX,
                  y: event.clientY,
                  videoLabel: channel.label,
                });
              }}
            />
            {overlayMode && overlayPanel && cameraLayout.editing && !singleHidden && (
              <OverlayHandles label={channel.label} panel={overlayPanel} selected={selectedOverlay === channel.label}
                operation={cameraLayout.operation} stageRef={gridRef} getVideo={() => channelRefs.current.get(channel.label) ?? null}
                onSelect={() => cameraLayout.setSelected(channel.label)}
                onChange={panel => cameraLayout.update(channel.label, panel)} />
            )}
          </div>
        );
      })}
    </div>
  );
}
