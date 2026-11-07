import { useCallback, useEffect, useRef, useState, type MutableRefObject, type PointerEvent, type MouseEvent } from "react";
import { constrainZoom, NORMAL_VIEW, zoomAt, type ZoomView } from "./pausedZoom";

export function usePausedZoom(videoRef: MutableRefObject<HTMLVideoElement | null>, enabled: boolean, source: string) {
  const [view, setView] = useState<ZoomView>(NORMAL_VIEW);
  const drag = useRef<{ id: number; clientX: number; clientY: number; view: ZoomView } | null>(null);
  const suppressClick = useRef(false);
  const dimensions = useCallback(() => {
    const video = videoRef.current;
    const box = video?.parentElement?.getBoundingClientRect();
    return { width: box?.width ?? 0, height: box?.height ?? 0, mediaWidth: video?.videoWidth ?? 0, mediaHeight: video?.videoHeight ?? 0 };
  }, [videoRef]);
  const changeZoom = useCallback((factor: number, anchor = { x: 0, y: 0 }) => {
    if (!enabled) return;
    const d = dimensions();
    setView(v => zoomAt(v, factor, anchor, d.width, d.height, d.mediaWidth, d.mediaHeight));
  }, [enabled, dimensions]);
  useEffect(() => {
    setView(NORMAL_VIEW);
    if (drag.current && videoRef.current?.hasPointerCapture(drag.current.id)) videoRef.current.releasePointerCapture(drag.current.id);
    drag.current = null; suppressClick.current = false;
  }, [enabled, source, videoRef]);
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !enabled) return;
    const wheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.metaKey) return;
      event.preventDefault(); event.stopPropagation();
      const box = video.parentElement!.getBoundingClientRect();
      changeZoom(Math.exp(-Math.max(-100, Math.min(100, event.deltaY)) * 0.003),
        { x: event.clientX - box.left - box.width / 2, y: event.clientY - box.top - box.height / 2 });
    };
    video.addEventListener("wheel", wheel, { passive: false });
    const resize = new ResizeObserver(() => {
      const d = dimensions();
      setView(v => constrainZoom(v, d.width, d.height, d.mediaWidth, d.mediaHeight));
    });
    resize.observe(video.parentElement!);
    return () => { video.removeEventListener("wheel", wheel); resize.disconnect(); };
  }, [videoRef, enabled, changeZoom, dimensions]);
  return {
    view: enabled ? view : NORMAL_VIEW,
    changeZoom,
    reset: () => setView(NORMAL_VIEW),
    onClickCapture: (event: MouseEvent) => {
      if (suppressClick.current) { event.stopPropagation(); event.preventDefault(); suppressClick.current = false; }
    },
    onPointerDown: (event: PointerEvent<HTMLVideoElement>) => {
      suppressClick.current = false;
      if (!enabled || view.scale <= 1 || event.button !== 0 || !event.isPrimary) return;
      drag.current = { id: event.pointerId, clientX: event.clientX, clientY: event.clientY, view };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    onPointerMove: (event: PointerEvent<HTMLVideoElement>) => {
      const start = drag.current;
      if (!start || start.id !== event.pointerId) return;
      if ((event.buttons & 1) === 0) { drag.current = null; return; }
      const dx = event.clientX - start.clientX, dy = event.clientY - start.clientY;
      if (Math.hypot(dx, dy) > 3) suppressClick.current = true;
      const d = dimensions();
      setView(constrainZoom({ ...start.view, x: start.view.x + dx, y: start.view.y + dy }, d.width, d.height, d.mediaWidth, d.mediaHeight));
    },
    onPointerUp: (event: PointerEvent<HTMLVideoElement>) => {
      if (drag.current?.id !== event.pointerId) return;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      drag.current = null;
    },
    onPointerCancel: () => { drag.current = null; suppressClick.current = false; },
    onLostPointerCapture: () => { drag.current = null; },
  };
}
