import { useEffect, useRef, type KeyboardEvent, type MouseEvent, type MutableRefObject, type PointerEvent as ReactPointerEvent } from "react";
import { moveOverlay, panOverlayCrop, resizeOverlay, type OverlayPanel } from "../../settings/overlayLayout";

interface Props {
  label: string;
  panel: OverlayPanel;
  selected: boolean;
  operation: "position" | "crop";
  stageRef: MutableRefObject<HTMLDivElement | null>;
  getVideo: () => HTMLVideoElement | null;
  onSelect: () => void;
  onChange: (panel: OverlayPanel) => void;
}

export function OverlayHandles({ label, panel, selected, operation, stageRef, getVideo, onSelect, onChange }: Props) {
  const drag = useRef<{
    id: number; x: number; y: number; panel: OverlayPanel; resize: boolean;
    stageWidth: number; stageHeight: number; width: number; height: number; mediaWidth: number; mediaHeight: number;
  } | null>(null);
  useEffect(() => () => { drag.current = null; }, []);

  function start(event: ReactPointerEvent<HTMLButtonElement>, resize: boolean) {
    if (event.button !== 0 || !event.isPrimary) return;
    const stage = stageRef.current?.getBoundingClientRect();
    const box = event.currentTarget.parentElement;
    if (!stage || !stage.width || !stage.height || !box) return;
    event.preventDefault(); event.stopPropagation();
    onSelect();
    const video = getVideo();
    drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY, panel, resize,
      stageWidth: stage.width, stageHeight: stage.height, width: video?.parentElement?.clientWidth ?? box.clientWidth, height: video?.parentElement?.clientHeight ?? box.clientHeight,
      mediaWidth: video?.videoWidth ?? 0, mediaHeight: video?.videoHeight ?? 0 };
    event.currentTarget.setPointerCapture(event.pointerId);
  }
  function move(event: ReactPointerEvent<HTMLButtonElement>) {
    const start = drag.current;
    if (!start || start.id !== event.pointerId) return;
    if ((event.buttons & 1) === 0) { finish(event); return; }
    event.preventDefault(); event.stopPropagation();
    const dx = event.clientX - start.x, dy = event.clientY - start.y;
    if (start.resize) onChange(resizeOverlay(start.panel, dx / start.stageWidth, dy / start.stageHeight));
    else if (operation === "position") onChange(moveOverlay(start.panel, dx / start.stageWidth, dy / start.stageHeight));
    else onChange({ ...start.panel, crop: panOverlayCrop(start.panel.crop, dx, dy, start.width, start.height, start.mediaWidth, start.mediaHeight) });
  }
  function finish(event: ReactPointerEvent<HTMLButtonElement>) {
    if (drag.current?.id !== event.pointerId) return;
    drag.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  }
  function keyboard(event: KeyboardEvent<HTMLButtonElement>, resize: boolean) {
    // Focused editor controls must not trigger the player's seek/play keys.
    event.stopPropagation();
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault(); event.stopPropagation(); onSelect();
    const step = event.shiftKey ? .05 : .01;
    const dx = event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0;
    const dy = event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0;
    if (resize) onChange(resizeOverlay(panel, dx, dy));
    else if (operation === "position") onChange(moveOverlay(panel, dx, dy));
    else onChange({ ...panel, crop: { ...panel.crop, x: Math.max(-1, Math.min(1, panel.crop.x + dx)), y: Math.max(-1, Math.min(1, panel.crop.y + dy)) } });
  }
  const handlers = {
    onPointerMove: move, onPointerUp: finish, onPointerCancel: finish,
    onLostPointerCapture: () => { drag.current = null; },
    onClick: (event: MouseEvent) => { event.stopPropagation(); onSelect(); },
    onDoubleClick: (event: MouseEvent) => event.stopPropagation(),
    onContextMenu: (event: MouseEvent) => { event.preventDefault(); event.stopPropagation(); },
  };
  return (
    <div className={`pointer-events-none absolute inset-0 rounded-md border-2 border-dashed ${selected ? "border-blue-400" : "border-white/60"}`}>
      <button type="button" {...handlers} onPointerDown={event => start(event, false)} onKeyDown={event => keyboard(event, false)}
        aria-label={`${operation === "crop" ? "Adjust crop of" : "Move"} ${label} overlay`}
        className="pointer-events-auto absolute inset-0 cursor-move touch-none focus-visible:outline-2 focus-visible:outline-blue-400">
        <span className="absolute left-1 top-1 rounded bg-black/80 px-2 py-1 text-xs text-white">{label} · {operation === "crop" ? "Video crop" : "Frame"}</span>
      </button>
      {operation === "position" && <button type="button" {...handlers} onPointerDown={event => start(event, true)} onKeyDown={event => keyboard(event, true)}
        aria-label={`Resize ${label} overlay`} title="Drag to resize; arrow keys also work"
        className="pointer-events-auto absolute -bottom-1 -right-1 flex h-6 w-6 cursor-se-resize touch-none items-center justify-center rounded border border-white bg-blue-600 text-white">
        <svg aria-hidden="true" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 13L13 3M7 13L13 7M11 13L13 11" /></svg>
      </button>}
    </div>
  );
}
