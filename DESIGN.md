# Trip Viewer — Design Document

An open-source, multi-channel, GPS-aware dashcam viewer with hardware-accelerated playback and integrated SD card import. Originally built for Wolf Box 3-channel dashcams; now also supports Thinkware 2-channel, Miltona MNCD60 single-channel, and a generic 4-channel format, with a modular parser architecture for adding more. MIT licensed.

**Repository:** [github.com/chrisl8/trip-viewer](https://github.com/chrisl8/trip-viewer)

---

## Problem Statement

Existing dashcam viewing software falls into two camps:

1. **Manufacturer apps** (Wolf Box, Thinkware, Viofo) — auto-detect their own multi-channel files and show them simultaneously with GPS, but have terrible UX: buried speed controls, no scrubbing, poor performance (software video decoding), and no interoperability.
2. **Third-party viewers** (Dashcam Viewer, DVPlayer, bbplay) — better UX and performance, but struggle with multi-channel support due to inconsistent file naming across manufacturers, proprietary GPS encodings, and a max of 2 simultaneous channels.

There is **no open-source, multi-channel, GPS-aware dashcam viewer** that uses hardware-accelerated video decoding.

---

## Competitive Landscape

| Feature                       | Wolf Box    | DCV              | DVPlayer   | bbplay    | **Trip Viewer** |
| ----------------------------- | ----------- | ---------------- | ---------- | --------- | --------------- |
| Multi-channel sync playback   | 3ch         | 2ch PiP          | 2-4ch      | n-ch      | **1–4ch**       |
| Live GPS map                  | yes         | yes              | yes        | yes       | **yes**         |
| Speed/heading display         | yes         | yes              | yes        | yes       | **yes**         |
| Variable playback speed       | buried      | yes              | yes        | yes       | **yes**         |
| Timeline scrubbing            | no          | yes              | yes        | yes       | **yes**         |
| Folder/batch loading          | no          | yes              | yes        | yes       | **yes**         |
| Trip segmentation             | no          | yes              | no         | no        | **yes**         |
| Hardware-accelerated decoding | no          | yes              | yes        | ?         | **yes**         |
| SD card import with verify    | no          | no               | no         | no        | **yes**         |
| Click-to-swap channels        | no          | no               | no         | no        | **yes**         |
| Pre-rendered timelapses       | no          | no               | no         | no        | **yes**         |
| Open source                   | no          | no               | no         | no        | **yes**         |

---

## Architecture

### What we built: Tauri v2 + React + HTML5 `<video>`

- **Tauri v2** — Rust backend, web frontend, ~3 MB installer. Uses the system WebView2 runtime (pre-installed on Windows 10/11) on Windows and WebKitGTK 4.1 on Linux, instead of bundling Chromium.
- **React 19 + TypeScript** — frontend with Zustand for state, Tailwind CSS v4 for styling.
- **N HTML5 `<video>` elements** — one per channel, synchronized via `requestVideoFrameCallback` against a stable master ref (front when present, otherwise the first channel). The grid adapts to the dashcam's channel count (1 for Miltona, 2 for Thinkware, 3 for Wolf Box, up to 4 for the generic format). Hardware-accelerated decoding via the browser's native HEVC decoder.
- **Leaflet + OpenStreetMap** — live GPS map with track polyline and interpolated vehicle marker. Auto-pans to follow the vehicle, but holds during user drag/zoom gestures.
- **Pure Rust container parsing** — `mp4` crate for metadata, custom binary parser for ShenShu GPS format. No ffprobe dependency.
- **Optional ffmpeg for timelapse** — the timelapse feature shells out to a user-supplied ffmpeg binary (configured in-app, not bundled) to pre-render fast-playback MP4s with GPS-driven variable speed. NVENC/NVDEC is used when available. Distinct from the rejected "ffprobe for metadata" decision below — playback and import remain ffmpeg-free; timelapse is opt-in.

### Why this architecture

HTML5 `<video>` provides hardware-accelerated HEVC decoding for free via WebView2 on Windows and via WebKitGTK's GStreamer backend on Linux. Three video elements can be synchronized well enough for dashcam playback (not frame-perfect, but within ~30ms — imperceptible for driving footage). The tradeoff is an OS-level codec dependency — Microsoft HEVC Video Extension on Windows, `gstreamer1.0-libav` + `plugins-bad` on Linux — which the app detects and handles with a startup gate.

### What was ruled out

- **Option C (Tauri + libmpv)** — `tauri-plugin-libmpv` is broken for multi-instance on Windows (only the first handle renders). Plugin has 9 stars, experimental. Would revisit only if upstream mpv fixes multi-instance.
- **Electron** — 100 MB runtime vs Tauri's 3 MB. No technical advantage for this use case.
- **PyQt + mpv** — Distribution is painful (PyInstaller), UI aesthetics harder than web tech.
- **ffprobe dependency** — Bundling ffprobe.exe adds ~80 MB, triggers Defender heuristics on unsigned builds, and PATH discovery on Windows is unreliable. Pure Rust `mp4` crate does everything needed.

### Accepted tradeoff: HEVC codec dependency

Wolf Box files are 100% HEVC. WebView2 on Windows plays HEVC only if the Microsoft HEVC Video Extension is installed (paid Store app on most consumer SKUs, free on OEM installs); WebKitGTK on Linux plays HEVC only if `gstreamer1.0-libav` is installed. The app handles both with a `<HevcSupportGate>` component that probes `canPlayType` at startup and shows platform-appropriate install guidance. A future Flatpak build would sidestep this entirely by bundling the `org.freedesktop.Platform.ffmpeg-full` extension. Transcoding to H.264 on import was considered and rejected — too slow and storage-heavy.

### Accepted tradeoff: multi-channel view is opt-in on Linux

On Windows the app renders front, interior, and rear simultaneously by default. On Linux the secondary channels (interior, rear) are **hidden by default and must be opted into** via the placeholder in the right column of the player or the `M` keyboard shortcut. The reason: on low-VRAM AMD iGPUs — notably Raven Ridge / Vega 11 with the common 1 GB UMA carveout — three concurrent HEVC pipelines through WebKitGTK's GStreamer+VAAPI path saturate memory bandwidth (`mclk` pegged at 100%), drive VRAM to 90%+, and in sustained playback can hang the GPU outright (full system lockup requiring reset). Single-channel playback on the same hardware still shows stutter and per-segment pipeline-rebuild stalls, but does not hang. Rather than try to detect "safe" GPU classes at runtime, we default-off on Linux and let the user enable multi-channel if their hardware handles it. Windows and macOS are unaffected — WebView2 and Safari's media path don't exhibit the same VRAM spike.

---

## Tech Stack

| Layer | Technology |
| ----- | ---------- |
| Framework | Tauri v2 (Rust backend, WebView2 on Windows / WKWebView on macOS / WebKitGTK 4.1 on Linux) |
| Frontend | React 19, TypeScript, Tailwind CSS v4 |
| State | Zustand |
| Maps | Leaflet + react-leaflet + OpenStreetMap tiles |
| Video sync | `requestVideoFrameCallback` API |
| Container parsing | `mp4` crate (pure Rust) |
| GPS decoding | Custom ShenShu MetaData binary parser (Wolf Box) + NovaTek `gps0` atom parser (Miltona) |
| Audio analysis | `symphonia` (pure Rust decoder, AAC / MP3 / ISO-MP4) for silence detection |
| File hashing | `sha2` crate (SHA-256, optimized in dev builds) |
| Tag + Place store | SQLite via `rusqlite` (bundled) + `rusqlite_migration` |
| File deletion | `trash` crate — OS recycle bin (recoverable) |
| Timelapse encoding | ffmpeg (user-supplied, opt-in) with NVENC + NVDEC when available |
| Parallelism | `rayon` (metadata probing, scan workers, parallel timelapse rebuild with bounded per-job memory) |
| Platform APIs | `windows-sys` on Windows (drive enumeration, disk space); `libc::statvfs` on Linux |
| Installer | NSIS on Windows, DMG on macOS (dual-arch), AppImage on Linux, via `tauri-action` (Flatpak planned) |
| Auto-update | `tauri-plugin-updater` with GitHub Releases |
| CI/CD | GitHub Actions (build on tag push, draft release) |

---

## Data Model

```
Trip
├── id: uuid
├── startTime: datetime
├── segments: Segment[]

Segment
├── id: uuid
├── startTime: datetime
├── durationS: f64
├── channels: Channel[]
│   ├── kind: "front" | "interior" | "rear"
│   ├── filePath: string
│   ├── resolution: { width, height }
│   ├── fps: f64
│   └── codec: string

GpsPoint
├── timestampS: f64
├── lat: f64
├── lon: f64
├── speedKmh: f64
├── heading: f64
```

File detection uses Wolf Box naming: `YYYY_MM_DD_HHMMSS_EE_C.MP4` where `EE` is event code and `C` is channel (F/I/R). Files are grouped into triplets by fuzzy timestamp matching (3-second window), then merged into trips by time gaps (120-second threshold).

---

## What's Been Built

### Playback

- **N-channel synchronized playback** — one `<video>` per channel, kept in lockstep via `requestVideoFrameCallback` with drift correction. Channel count adapts to the dashcam (1 to 4).
- **Click-to-swap layout** — click a side video to promote it to the main position; videos stay playing during swap (stable DOM, CSS-only repositioning)
- **Fullscreen main video** — double-click the main panel to enter fullscreen (browser Fullscreen API), Escape to exit
- **Transport controls** — play/pause, seek ±5s/±30s, speed (0.5x/1x/2x/4x/8x), source mode picker (originals vs. timelapse tier)
- **Keyboard shortcuts** — Space, arrows, Shift+arrows, brackets for speed, D for drift HUD, M to enable multi-channel on Linux
- **Segment auto-advance** — continuous playback across multi-segment trips
- **HEVC support gate** — startup check with Store deep-link on Windows / apt-install hint on Linux if HEVC decoder is missing

### GPS & Map

- **Live GPS map** — OpenStreetMap with Leaflet, track polyline drawn as video plays
- **Interpolated vehicle marker** — smooth position updates between GPS samples
- **Speed & heading HUD** — real-time readouts overlaid on the map panel; heading holds last moving direction when stopped, speed snaps to 0 at full stop
- **Auto-pan with gesture release** — map follows the vehicle, but holds in place during user drag/zoom so you can inspect a moment without being yanked back
- **Custom GPS parsers** — reverse-engineered ShenShu MetaData (Wolf Box) and NovaTek `gps0` atom (Miltona) binary formats

### Library & file management

- **Multi-format dashcam support** — Wolf Box 3-channel, Thinkware 2-channel, Miltona MNCD60 single-channel, generic 4-channel. Modular parser architecture in `scan/naming.rs` for adding more.
- **Folder scanner** — recursive MP4 discovery, per-format filename parsing, fuzzy triplet matching, trip grouping. Skips `Timelapses/` and dot-directories on rescan.
- **SD card import** — full pipeline: discover removable drives → stage with SHA-256 verification → wipe source → distribute to Videos/Photos. Duplicate detection, collision handling, unknown file prompts, cancel support, interrupt safety, lock file with PID recovery, logging with 30-day rotation
- **Import from a folder** — non-destructive variant of the same pipeline for files already on disk; no source wipe
- **Import progress UI** — live progress bar with speed, file counter, phase indicators, cancel button; events throttled to ~15/sec to avoid IPC saturation
- **Trip operations** — delete a whole trip (originals, timelapses, tags, optional source folder), or mark two or more trips and merge them manually. Merge dialog has a strategy picker for the surviving trip's timelapses (concatenate vs. drop and rebuild).
- **Storage usage** — sidebar surfaces total bytes used and reclaimable bytes (originals that can be dropped now that timelapses exist), with a one-click filter

### Tagging & review

- **Auto-tagging scan pipeline** — background scan tags segments as `event` (dashcam flag), `stationary` (GPS-derived), `silent`/`no_audio` (Symphonia-driven audio analysis), and place matches. Per-trip × per-scan coverage matrix in the Scan view shows what each scan run touched.
- **Places** — saved named locations (lat/lon + radius), set manually or with one click from the player. Auto-tag matching segments on the next scan.
- **Review view** — full-library table with tag-based filtering and bulk actions (Keep, bulk delete). Bulk actions are scoped to the intersection of selection and filter so the action button always reflects what will run.
- **In-player selection mode** — timeline-driven multi-select with shift-click ranges and a single bulk-delete that trashes every channel file
- **Issues view** — classified triage list for files the scanner couldn't ingest (invalid filename, unreadable, missing `moov`, corrupt boxes, no video track, other) with reveal-in-folder, copy-path, move-to-trash, and filter-gated bulk delete. Tab only appears when there's something to triage.
- **Recoverable deletes** — everything goes through the `trash` crate to the OS recycle bin

### Timelapse

- **Pre-render pipeline** — ffmpeg-driven background encoder that produces 8× / 16× / 60× fast-playback MP4s per (trip, tier, channel). 8× is constant; 16× and 60× use GPS-derived event detection (hard braking, hard acceleration, sharp turns) plus the dashcam's own event flag to slow down through interesting moments.
- **NVENC + NVDEC path** — used automatically when the configured ffmpeg binary reports the capability
- **Parallel rebuild** — multi-job rebuild with bounded per-job memory so a "rebuild all" pass doesn't oversubscribe the GPU
- **Timelapse Library view** — per-trip status table with per-trip Rebuild button and scope picker (new & unfinished / retry failed / rebuild all)
- **Originals-as-cache** — timelapses are an archival format, not a cache. Deleting originals leaves timelapses; the trip stays in the library and plays from the timelapse tier. Only "Delete trip…" removes everything.
- **Console-window suppression** — installed Windows builds suppress the ffmpeg console window flash on every invocation

### Distribution

- **NSIS installer** — ~3 MB Windows setup exe
- **DMG installer** — signed and notarized, dual-arch (Intel + Apple Silicon) via CI matrix
- **AppImage** — single-file Linux binary; CI bundles WebKitGTK + GStreamer codec plugins via `linuxdeploy`
- **Flatpak (Flathub)** — planned; would ship a sandboxed Linux package using `org.freedesktop.Platform.ffmpeg-full` for codec support with Flathub handling updates. Not in this release.
- **GitHub Actions CI** — auto-build on version tag, draft release with installer + updater manifest
- **Auto-updater** — checks GitHub Releases on startup for NSIS, DMG, and AppImage builds (would be disabled in Flatpak when added, since Flathub manages updates)
- **Tauri signing keys** — update artifacts signed for integrity verification

---

## Future Ideas

### Near-term (would use now)

- **Audio source selection** — see which channel provides audio, switch it to a different channel
- **Flip camera view** — mirror a video horizontally, persist the preference

### Medium-term (polish and generalize)

- **More dashcam parsers** — Viofo, BlackVue, GoPro on top of the existing Wolf Box / Thinkware / Miltona / Generic 4-channel set. Modular naming-parser architecture is already in place; each addition is small.
- **Speed/altitude/g-force graphs** — if accelerometer data is available in the GPS stream
- **Clip export** — select a time range, export to a new MP4
- **Snapshot capture** — save a frame as an image
- **GPX/KML export** — export GPS tracks for use in mapping tools
- **Bookmarking** — flag moments on the timeline for later review
- **Settings UI** — currently scattered (ffmpeg path lives in the Timelapse modal). Consolidate, plus add preferred map tile source, units (mph/kmh), default playback speed.

### Long-term (analysis and automation)

- **Trip journal / map** — all trips plotted on a world map, click to jump to footage
- **Scene change detection** — thumbnail timeline of interesting moments beyond the existing GPS-derived event detection
- **Audio spike detection** — flag horn honks, crash sounds
- **Object detection** — YOLO on keyframes (vehicles, people, signs)
- **OCR** — extract text from frames (speed limit signs, license plates)
- **AI-powered search** — "find the clip where I passed the red barn" (local vision model)
- **Speed overlay** — bake GPS speed into exported clips
- **OpenStreetMap contribution** — extract GPS traces and frames for mappers
