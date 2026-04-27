import { convertFileSrc } from "@tauri-apps/api/core";

// Windows (WebView2) uses Tauri's asset protocol via convertFileSrc.
// Linux (WebKitGTK) and macOS (WKWebView) can't — see VideoGrid.tsx
// for the full rationale — so they run a loopback HTTP server and
// get a non-zero videoPort from the backend.
export function videoSrcFor(filePath: string, videoPort: number | null): string {
  if (videoPort && videoPort > 0) {
    return `http://127.0.0.1:${videoPort}${encodeURI(filePath)}`;
  }
  return convertFileSrc(filePath);
}
