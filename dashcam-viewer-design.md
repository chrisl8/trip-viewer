# Trip Viewer — Design Document

An open-source, multi-channel, GPS-aware dashcam viewer with hardware-accelerated playback and integrated SD card import. Built for Wolf Box 3-channel dashcams, designed to be extensible to other manufacturers. MIT licensed.

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
| Multi-channel sync playback   | 3ch         | 2ch PiP          | 2-4ch      | n-ch      | **3ch**         |
| Live GPS map                  | yes         | yes              | yes        | yes       | **yes**         |
| Speed/heading display         | yes         | yes              | yes        | yes       | **yes**         |
| Variable playback speed       | buried      | yes              | yes        | yes       | **yes**         |
| Timeline scrubbing            | no          | yes              | yes        | yes       | **yes**         |
| Folder/batch loading          | no          | yes              | yes        | yes       | **yes**         |
| Trip segmentation             | no          | yes              | no         | no        | **yes**         |
| Hardware-accelerated decoding | no          | yes              | yes        | ?         | **yes**         |
| SD card import with verify    | no          | no               | no         | no        | **yes**         |
| Click-to-swap channels        | no          | no               | no         | no        | **yes**         |
| Open source                   | no          | no               | no         | no        | **yes**         |

---

## Architecture

### What we built: Tauri v2 + React + HTML5 `<video>`

- **Tauri v2** — Rust backend, web frontend, ~3 MB installer. Uses the system WebView2 runtime (pre-installed on Windows 10/11) instead of bundling Chromium.
- **React 19 + TypeScript** — frontend with Zustand for state, Tailwind CSS v4 for styling.
- **3x HTML5 `<video>` elements** — synchronized via `requestVideoFrameCallback` for frame-accurate sync across front/interior/rear channels. Hardware-accelerated decoding via the browser's native HEVC decoder.
- **Leaflet + OpenStreetMap** — live GPS map with track polyline and interpolated vehicle marker.
- **Pure Rust container parsing** — `mp4` crate for metadata, custom binary parser for ShenShu GPS format. No ffprobe dependency.

### Why this architecture

HTML5 `<video>` provides hardware-accelerated HEVC decoding for free via WebView2. Three video elements can be synchronized well enough for dashcam playback (not frame-perfect, but within ~30ms — imperceptible for driving footage). The tradeoff is requiring the Microsoft HEVC Video Extension on Windows, which the app detects and handles with a startup gate.

### What was ruled out

- **Option C (Tauri + libmpv)** — `tauri-plugin-libmpv` is broken for multi-instance on Windows (only the first handle renders). Plugin has 9 stars, experimental. Would revisit only if upstream mpv fixes multi-instance.
- **Electron** — 100 MB runtime vs Tauri's 3 MB. No technical advantage for this use case.
- **PyQt + mpv** — Distribution is painful (PyInstaller), UI aesthetics harder than web tech.
- **ffprobe dependency** — Bundling ffprobe.exe adds ~80 MB, triggers Defender heuristics on unsigned builds, and PATH discovery on Windows is unreliable. Pure Rust `mp4` crate does everything needed.

### Accepted tradeoff: HEVC Extension

Wolf Box files are 100% HEVC. Windows WebView2 can play HEVC but only if the Microsoft HEVC Video Extension is installed (paid Store app on most consumer SKUs, free on OEM installs). The app handles this with a `<HevcSupportGate>` component that probes `canPlayType` at startup and deep-links to the Store if missing. Transcoding to H.264 on import was considered and rejected — too slow and storage-heavy.

---

## Tech Stack

| Layer | Technology |
| ----- | ---------- |
| Framework | Tauri v2 (Rust backend, WebView2 frontend) |
| Frontend | React 19, TypeScript, Tailwind CSS v4 |
| State | Zustand |
| Maps | Leaflet + react-leaflet + OpenStreetMap tiles |
| Video sync | `requestVideoFrameCallback` API |
| Container parsing | `mp4` crate (pure Rust) |
| GPS decoding | Custom ShenShu MetaData binary parser (NMEA DDMM.MMMM format) |
| File hashing | `sha2` crate (SHA-256, optimized in dev builds) |
| Windows APIs | `windows-sys` (drive enumeration, disk space) |
| Installer | NSIS via `tauri-action` |
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

- **3-channel synchronized playback** — front/interior/rear play in lockstep via `requestVideoFrameCallback` with drift correction
- **Click-to-swap layout** — click a side video to promote it to the main position; videos stay playing during swap (stable DOM, CSS-only repositioning)
- **Fullscreen main video** — click the main panel to enter fullscreen (browser Fullscreen API), Escape to exit
- **Transport controls** — play/pause, seek ±5s/±30s, speed (0.5x/1x/2x/4x/8x)
- **Keyboard shortcuts** — Space, arrows, Shift+arrows, brackets for speed, D for drift HUD
- **Segment auto-advance** — continuous playback across multi-segment trips
- **HEVC support gate** — startup check with Store deep-link if HEVC decoder is missing

### GPS & Map

- **Live GPS map** — OpenStreetMap with Leaflet, track polyline drawn as video plays
- **Interpolated vehicle marker** — smooth position updates between GPS samples
- **Speed & heading HUD** — real-time readouts overlaid on the map panel
- **Custom GPS parser** — reverse-engineered ShenShu MetaData binary format from Wolf Box firmware

### File Management

- **Folder scanner** — recursive MP4 discovery, Wolf Box filename parsing, fuzzy triplet matching, trip grouping
- **Remember last folder** — auto-reopens on next launch via localStorage
- **SD card import** — full pipeline: discover removable drives → stage with SHA-256 verification → wipe source → distribute to Videos/Photos. Duplicate detection, collision handling, unknown file prompts, cancel support, interrupt safety, lock file, logging with 30-day rotation
- **Import progress UI** — live progress bar with speed, file counter, phase indicators, cancel button
- **Import summary** — completion dialog with per-source stats and date range of imported footage

### Distribution

- **NSIS installer** — ~3 MB Windows setup exe
- **GitHub Actions CI** — auto-build on version tag, draft release with installer + updater manifest
- **Auto-updater** — checks GitHub Releases on startup, one-click update and restart
- **Tauri signing keys** — update artifacts signed for integrity verification

---

## Future Ideas

### Near-term (would use now)

- **Audio source selection** — see which channel provides audio, switch it to a different channel
- **Flip camera view** — mirror a video horizontally, persist the preference
- **Error file review** — inspect files that failed to parse, see what's wrong, delete from the interface

### Medium-term (polish and generalize)

- **Camera plugin system** — camera-specific parsers as plugins (Viofo, BlackVue, GoPro, generic fallback)
- **Speed/altitude/g-force graphs** — if accelerometer data is available in the GPS stream
- **Clip export** — select a time range, export to a new MP4
- **Snapshot capture** — save a frame as an image
- **GPX/KML export** — export GPS tracks for use in mapping tools
- **Bookmarking** — flag moments on the timeline for later review
- **Settings** — preferred map tile source, units (mph/kmh), default playback speed

### Long-term (analysis and automation)

- **Trip journal / map** — all trips plotted on a world map, click to jump to footage
- **Batch GPS extraction** — process entire folder → trip map overview
- **Scene change detection** — thumbnail timeline of interesting moments
- **Audio spike detection** — flag horn honks, crash sounds
- **Object detection** — YOLO on keyframes (vehicles, people, signs)
- **OCR** — extract text from frames (speed limit signs, license plates)
- **AI-powered search** — "find the clip where I passed the red barn" (local vision model)
- **Timelapse generation** — from full trip footage
- **Speed overlay** — bake GPS speed into exported clips
- **OpenStreetMap contribution** — extract GPS traces and frames for mappers
