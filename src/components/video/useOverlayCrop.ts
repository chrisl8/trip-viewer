import { useEffect, useState, type CSSProperties, type MutableRefObject } from "react";
import { cropGeometry, type OverlayCrop, type OverlayFrame } from "../../settings/overlayLayout";
import { croppedVideoMask, overlayMaskStyle } from "./overlayMask";

// Only measure on resize/metadata changes, never on playback frames. The same
// video element remains mounted; crop changes do not touch its source or clock.
export function useOverlayCrop(ref: MutableRefObject<HTMLVideoElement | null>, crop: OverlayCrop | undefined, frame: OverlayFrame = "rounded"): CSSProperties | undefined {
  const [size, setSize] = useState({ width: 0, height: 0, mediaWidth: 0, mediaHeight: 0 });
  const enabled = crop !== undefined;
  useEffect(() => {
    const video = ref.current;
    const box = video?.parentElement;
    if (!enabled || !video || !box) return;
    const measure = () => setSize({ width: box.clientWidth, height: box.clientHeight, mediaWidth: video.videoWidth, mediaHeight: video.videoHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    video.addEventListener("loadedmetadata", measure);
    return () => { observer.disconnect(); video.removeEventListener("loadedmetadata", measure); };
  }, [enabled, ref]);
  if (!crop) return undefined;
  const geometry = cropGeometry(crop, size.width, size.height, size.mediaWidth, size.mediaHeight);
  return geometry ? { position: "absolute", left: geometry.left, top: geometry.top, width: geometry.width, height: geometry.height, maxWidth: "none",
    ...croppedVideoMask(geometry, size.width, size.height, frame) }
    : { width: "100%", height: "100%", objectFit: "cover", ...overlayMaskStyle(frame) };
}
