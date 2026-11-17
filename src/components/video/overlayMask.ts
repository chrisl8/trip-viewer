import type { CSSProperties } from "react";
import type { OverlayFrame } from "../../settings/overlayLayout";

export function overlayBorderRadius(frame: OverlayFrame): string {
  return frame === "mirror" ? "18% 18% 24% 24% / 35% 35% 45% 45%" : "10px";
}

export function overlayMaskStyle(frame: OverlayFrame): CSSProperties {
  const radius = overlayBorderRadius(frame);
  const clip = `inset(0 round ${radius})`;
  return {
    borderRadius: radius,
    clipPath: clip,
    WebkitClipPath: clip,
    // WebKitGTK can otherwise let its composited video plane escape a rounded
    // overflow container. Force an alpha-masked surface as well as clipping.
    WebkitMaskImage: "-webkit-radial-gradient(white, black)",
    isolation: "isolate",
    transform: "translateZ(0)",
  };
}

interface CropRectangle { width: number; height: number; left: number; top: number }

export function cropMaskInsets(rect: CropRectangle, width: number, height: number) {
  // Clip in the oversized video's own coordinates. Its crop moves, while this
  // visible window must remain aligned with the stationary mirror/frame.
  return {
    top: Math.max(0, -rect.top),
    right: Math.max(0, rect.width + rect.left - width),
    bottom: Math.max(0, rect.height + rect.top - height),
    left: Math.max(0, -rect.left),
  };
}

export function croppedVideoMask(rect: CropRectangle, width: number, height: number, frame: OverlayFrame): CSSProperties {
  const inset = cropMaskInsets(rect, width, height);
  const px = (value: number) => `${Number(value.toFixed(4))}px`;
  // Percent radii on an enlarged video would scale with the entire video,
  // rather than the mirror window, so convert the frame radii to pixels here.
  const radius = frame === "mirror"
    ? `${[.18, .18, .24, .24].map(factor => px(width * factor)).join(" ")} / ${[.35, .35, .45, .45].map(factor => px(height * factor)).join(" ")}`
    : "10px";
  const clip = `inset(${px(inset.top)} ${px(inset.right)} ${px(inset.bottom)} ${px(inset.left)} round ${radius})`;
  return { clipPath: clip, WebkitClipPath: clip };
}
