export interface ZoomView { scale: number; x: number; y: number }
export const NORMAL_VIEW: ZoomView = { scale: 1, x: 0, y: 0 };

export function constrainZoom(view: ZoomView, width: number, height: number, mediaWidth = width, mediaHeight = height): ZoomView {
  if (width <= 0 || height <= 0 || mediaWidth <= 0 || mediaHeight <= 0) return NORMAL_VIEW;
  const scale = Math.max(1, Math.min(8, Number.isFinite(view.scale) ? view.scale : 1));
  const fit = Math.min(width / mediaWidth, height / mediaHeight);
  const maxX = Math.max(0, (mediaWidth * fit * scale - width) / 2);
  const maxY = Math.max(0, (mediaHeight * fit * scale - height) / 2);
  const clamp = (v: number, max: number) => Math.max(-max, Math.min(max, Number.isFinite(v) ? v : 0));
  return { scale, x: clamp(view.x, maxX), y: clamp(view.y, maxY) };
}
export function zoomAt(view: ZoomView, factor: number, anchor: { x: number; y: number }, width: number, height: number, mediaWidth = width, mediaHeight = height): ZoomView {
  const scale = Math.max(1, Math.min(8, view.scale * factor));
  const ratio = scale / view.scale;
  return constrainZoom({ scale, x: anchor.x - (anchor.x - view.x) * ratio, y: anchor.y - (anchor.y - view.y) * ratio }, width, height, mediaWidth, mediaHeight);
}
