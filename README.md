<img src="icon/icon-128.png" align="left" width="96" alt="Trip Viewer icon"/>

# Trip Viewer

**A free, open-source dashcam viewer with synchronized video and a live GPS map.**

<a href="#windows"><img src="icon/badges/windows.svg" alt="Windows" height="28"/></a> &nbsp; <a href="#macos"><img src="icon/badges/apple.svg" alt="macOS" height="28"/></a> &nbsp; <a href="#linux"><img src="icon/badges/linux.svg" alt="Linux" height="28"/></a>

<br clear="left"/>

Trip Viewer plays back footage from dashcams with one to four camera channels, keeping every channel perfectly in sync. A live OpenStreetMap view tracks your vehicle's GPS position as the video plays, with speed and heading shown on the map — a favorite feature among single-camera dashcam owners as well as multi-channel setups.

Hardware video decoding keeps playback smooth even at high resolution, and Trip Viewer runs as a lightweight native app on Windows, macOS, and Linux (~3 MB installer on Windows, signed DMG on macOS, AppImage on Linux).

![Trip Viewer screenshot showing 3-channel synchronized playback with GPS map](screenshot.png)

## How to install

**Trip Viewer runs on Windows 10/11, macOS 11 (Big Sur) or later, and modern Linux distributions (tested on Ubuntu 22.04+).** No developer tools required.

### Windows

1. Go to the [Releases page](https://github.com/chrisl8/trip-viewer/releases)
2. Under the latest release, download the file ending in **`_x64-setup.exe`**
3. Run the installer — Windows may show a SmartScreen warning since the app is new and unsigned. Click **"More info"** then **"Run anyway"**
4. Launch **Trip Viewer** from your Start Menu

**One extra requirement on Windows:** Your dashcam probably records in HEVC (H.265) format. Windows needs a decoder for this. Trip Viewer will check on startup and link you to the Microsoft Store if it's missing. The [HEVC Video Extension](https://apps.microsoft.com/detail/9n4wgh0z6vhq) is a one-time install.

### macOS

1. Go to the [Releases page](https://github.com/chrisl8/trip-viewer/releases)
2. Check which chip your Mac has: **Apple menu → About This Mac**
   - **Apple M1 / M2 / M3 / M4** (or later) → download the file ending in **`_aarch64.dmg`**
   - **Intel** → download the file ending in **`_x64.dmg`**
3. Double-click the DMG to mount it, then drag **Trip Viewer** into the **Applications** folder
4. Launch **Trip Viewer** from Launchpad or Applications

The macOS build is code-signed and notarized by Apple, so you won't see Gatekeeper warnings on first launch. HEVC playback works out of the box — no codec extensions to install.

### Linux

1. Go to the [Releases page](https://github.com/chrisl8/trip-viewer/releases)
2. Under the latest release, download the file ending in **`.AppImage`**
3. Make it executable: `chmod +x trip-viewer_*_amd64.AppImage`
4. Run it: `./trip-viewer_*_amd64.AppImage`

**One extra requirement on Linux:** HEVC playback needs GStreamer's libav plugin. Trip Viewer will check on startup and show an install hint if it's missing. On Debian/Ubuntu:

```bash
sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-bad
```

By default on Linux, only the primary channel is shown — press **M** to enable multi-channel view. This is opt-in because on some older integrated GPUs, multiple concurrent HEVC streams can overwhelm video memory. On typical modern hardware it works fine.

## What it does

- **Multi-channel synchronized playback (1–4 channels)** — every camera on your dashcam plays in lockstep. Click a side view to make it the main view. Double-click the main view for fullscreen.
- **Live GPS map** — an OpenStreetMap view tracks your vehicle position in real time as the video plays, with a trail showing where you've been.
- **Speed and heading display** — real-time readouts overlaid on the map so you can see how fast you were going at any moment.
- **Timeline with speed graph** — scrub through footage visually. The speed graph shows interesting moments (hard braking, acceleration) so you can jump right to them.
- **SD card import** — pull footage directly off your dashcam's SD card. Files are copied with SHA-256 integrity verification, then organized into your library. The SD card is wiped after a successful verified transfer, ready to go back in your dashcam.
- **Trip detection** — automatically groups your footage into trips based on recording timestamps. No manual organization needed.
- **Keyboard shortcuts** — Space to play/pause, arrow keys to seek, brackets to change speed. Click "Keyboard shortcuts" in the sidebar footer for the full list.
- **Auto-updates** — the app checks for new versions on startup and offers a one-click update.

## Supported dashcams

Trip Viewer auto-detects common dashcam filename formats at import time. You just point it at a folder of video files — no renaming, no manual configuration.

Currently recognized formats:

- **Wolf Box** (3-channel: front / interior / rear) — filenames like `2026_03_15_173951_02_F.MP4`. Full GPS support via the ShenShu metadata parser.
- **Thinkware** (2-channel: front / rear) — filenames like `REC_2026_03_06_07_25_52_F.MP4` or `EVT_...` for event recordings. SD cards with Thinkware folder structure (`cont_rec/`, `evt_rec/`, etc.) are auto-detected at import. The tested Thinkware model does not record GPS, so the map panel is hidden and replaced with a compact caption — no wasted screen space. If a GPS-equipped Thinkware model turns up, GPS support can be added.
- **Miltona MNCD60** (single-channel) — filenames like `FILE211202-151504-000406F.MOV`. GPS support via the proprietary `gps0` atom (NovaTek-family chipset), with speed readout from the embedded km/h byte. GPS coordinates were ground-truthed against seven on-screen overlay readings from a reference clip.
- **Generic 4-channel** (best-effort) — filenames like `2026_03_06_072552_A.MP4` through `..._D.MP4` (or `_1` through `_4`). Labeled "Channel A" through "Channel D". GPS not yet implemented for this format.

If your dashcam uses a different naming convention, [open an issue](https://github.com/chrisl8/trip-viewer/issues) with a few example filenames (and ideally a sample file) and I'll add support. The parser architecture is modular and new format support is a small, low-risk addition.

## Built with AI

This project was built with significant help from [Claude Code](https://claude.ai/claude-code) (Anthropic's AI coding assistant). I'm a full-time software developer, and Claude Code was an excellent collaborator — it helped with architecture decisions, wrote the Rust backend and React frontend, reverse-engineered the dashcam GPS format, and built the entire SD card import pipeline. The result is a codebase I understand fully and maintain myself, with AI as a force multiplier.

If you're curious about how it was built, the [DESIGN.md](DESIGN.md) document covers the architecture decisions and tech stack in detail.

## Feature requests and bug reports

I actively maintain this project and I'm interested in making it better. If you:

- **Found a bug** — [open an issue](https://github.com/chrisl8/trip-viewer/issues) with what happened and what you expected
- **Want a feature** — [open an issue](https://github.com/chrisl8/trip-viewer/issues) describing what you'd like. Some ideas I'm already thinking about: audio source selection, clip export, GPX track export, camera view flipping, and AI-powered footage search
- **Have a different dashcam** — I'd love to add support for it. Open an issue with your dashcam model and, if possible, a sample file

### If an update breaks things

If a new release breaks something for you, uninstall it via your OS and reinstall the previous version from the [Releases page](https://github.com/chrisl8/trip-viewer/releases). Please also file a bug report. Auto-update prompts can be dismissed if you want to stay on a working version.

## Development

If you want to build Trip Viewer from source or contribute:

### Prerequisites

- Node.js 20+
- Rust 1.70+ (via [rustup](https://rustup.rs/))
- **Windows:** HEVC Video Extension (see [Windows install](#windows) above)
- **macOS:** Xcode Command Line Tools (`xcode-select --install`). HEVC playback works natively via AVFoundation — no extra codecs needed. Local `npm run tauri build` produces a DMG for the host architecture only; CI uses a matrix build for both Intel and Apple Silicon.
- **Linux:** `webkit2gtk-4.1`, `gstreamer1.0-libav`, `gstreamer1.0-plugins-bad`, plus Tauri's standard build deps (see [Tauri prerequisites](https://tauri.app/start/prerequisites/))

### Build and run

```bash
npm install
npm run tauri dev      # Development mode (hot-reload)
npm run tauri build    # Production build (creates installer)
```

First build compiles the Rust backend (~2 minutes). Subsequent builds use incremental compilation (~10 seconds).

### Tech stack

| Layer             | Technology                                                                                 |
| ----------------- | ------------------------------------------------------------------------------------------ |
| App framework     | Tauri v2 (Rust backend, WebView2 on Windows / WKWebView on macOS / WebKitGTK 4.1 on Linux) |
| Frontend          | React 19, TypeScript, Tailwind CSS v4, Zustand                                             |
| Maps              | Leaflet + react-leaflet + OpenStreetMap                                                    |
| Video sync        | `requestVideoFrameCallback` API                                                            |
| Container parsing | `mp4` crate (pure Rust, no ffprobe)                                                        |
| GPS decoding      | Custom ShenShu MetaData (Wolf Box) + NovaTek gps0 atom (Miltona) parsers                   |
| File hashing      | SHA-256 via `sha2` crate                                                                   |
| CI/CD             | GitHub Actions + NSIS (Windows) + DMG (macOS, dual-arch) + AppImage (Linux) + auto-updater |

See [DESIGN.md](DESIGN.md) for architecture decisions and [RELEASING.md](RELEASING.md) for release instructions.

## License

[MIT](LICENSE)
