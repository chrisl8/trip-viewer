# Trip Viewer

An open-source, multi-channel dashcam viewer with synchronized playback and live GPS tracking.

Built for Wolf Box 3-channel dashcams (front/interior/rear), but designed to be extensible to other manufacturers.

## Features

- **3-channel synchronized playback** — front, interior, and rear cameras play in lockstep via `requestVideoFrameCallback`
- **Live GPS map** — OpenStreetMap with track polyline and interpolated vehicle marker
- **Speed & heading HUD** — real-time readouts overlaid on the map
- **Timeline scrubber** — SVG timeline with speed graph, segment bars, click-to-seek across segments
- **Keyboard shortcuts** — Space (play/pause), arrows (seek), brackets (speed), D (drift HUD)
- **Folder scanner** — auto-detects triplets with fuzzy timestamp matching, groups into trips
- **GPS extraction** — reverse-engineered ShenShu MetaData binary format (NMEA coordinates)
- **Segment auto-advance** — continuous playback across multi-segment trips
- **Drift debugging** — toggleable HUD showing interior/rear sync drift in milliseconds

## Prerequisites

- **Node.js** 20+
- **Rust** 1.70+ (via [rustup](https://rustup.rs/))
- **Microsoft HEVC Video Extension** — required for HEVC playback on Windows. Install from the [Microsoft Store](ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ) or search "HEVC Video Extensions" in the Store app.

## Getting started

```bash
# Install JS dependencies
npm install

# Run in development mode (compiles Rust backend + starts Vite dev server)
npm run tauri dev

# Build for production
npm run tauri build
```

On first run, `npm run tauri dev` will compile the Rust backend (~1-2 minutes). Subsequent runs use incremental compilation (~5-10 seconds).

## Usage

1. Click **Open folder** and select a directory containing dashcam MP4 files
2. The scanner groups files into trips (files named `YYYY_MM_DD_HHMMSS_EE_C.MP4`)
3. Select a trip from the sidebar
4. Press **Space** or click **Play** to start synchronized playback

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| Space | Play / Pause |
| Left / Right | Seek 5 seconds |
| Shift + Left / Right | Seek 30 seconds |
| `[` / `]` | Decrease / Increase speed |
| D | Toggle drift HUD |

## Tech stack

- **Frontend**: React 19, TypeScript, Tailwind CSS v4, Zustand, Leaflet
- **Backend**: Tauri v2, Rust
- **Video**: HTML5 `<video>` with `requestVideoFrameCallback` for frame-accurate sync
- **Container parsing**: `mp4` crate (pure Rust, no ffprobe dependency)
- **GPS**: Custom ShenShu MetaData binary parser (NMEA DDMM.MMMM format)

## License

[MIT](LICENSE)
