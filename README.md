# Trip Viewer

**A free, open-source dashcam viewer that actually works.**

If you own a multi-channel dashcam (like a Wolf Box, Viofo, or similar), you've probably been disappointed by the software that comes with it. The manufacturer apps are slow, clunky, and can barely scrub through footage. The paid third-party viewers are better but still can't handle three camera channels well. And none of them are open source.

Trip Viewer changes that. It plays all three of your dashcam channels — front, interior, and rear — perfectly synchronized, with a live GPS map tracking your position as the video plays. It uses your computer's hardware video decoder, so playback is smooth even at high resolution. And it runs as a lightweight native Windows app (~3 MB installer), not a bloated Electron app.

## How to install

**Trip Viewer runs on Windows 10 and Windows 11.** No developer tools required.

1. Go to the [Releases page](https://github.com/chrisl8/trip-viewer/releases)
2. Under the latest release, download the file ending in **`_x64-setup.exe`**
3. Run the installer — Windows may show a SmartScreen warning since the app is new and unsigned. Click **"More info"** then **"Run anyway"**
4. Launch **Trip Viewer** from your Start Menu

**One extra requirement:** Your dashcam probably records in HEVC (H.265) format. Windows needs a decoder for this. Trip Viewer will check on startup and link you to the Microsoft Store if it's missing. The [HEVC Video Extension](ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ) is a one-time install.

Trip Viewer auto-checks for updates on launch, so you'll always have the latest version.

## What it does

- **3-channel synchronized playback** — front, interior, and rear cameras play in lockstep. Click a side view to make it the main view. Double-click the main view for fullscreen.
- **Live GPS map** — an OpenStreetMap view tracks your vehicle position in real time as the video plays, with a trail showing where you've been.
- **Speed and heading display** — real-time readouts overlaid on the map so you can see how fast you were going at any moment.
- **Timeline with speed graph** — scrub through footage visually. The speed graph shows interesting moments (hard braking, acceleration) so you can jump right to them.
- **SD card import** — pull footage directly off your dashcam's SD card. Files are copied with SHA-256 integrity verification, then organized into your library. The SD card is wiped after a successful verified transfer, ready to go back in your dashcam.
- **Trip detection** — automatically groups your footage into trips based on recording timestamps. No manual organization needed.
- **Keyboard shortcuts** — Space to play/pause, arrow keys to seek, brackets to change speed. Click "Keyboard shortcuts" in the sidebar footer for the full list.
- **Auto-updates** — the app checks for new versions on startup and offers a one-click update.

## Currently supported dashcams

Trip Viewer was built and tested with **Wolf Box 3-channel dashcams** (front/interior/rear with `_F/_I/_R` file naming). The GPS parser handles the ShenShu metadata format used by Wolf Box firmware.

The architecture is designed to support other manufacturers — the file scanner, GPS parser, and channel mapping are all modular. If you have a different dashcam and want to try it, [open an issue](https://github.com/chrisl8/trip-viewer/issues) with details about your dashcam model and file format. I'm happy to add support for other cameras.

## Platform support

Trip Viewer currently runs on **Windows only** (10 and 11). The technology it's built on (Tauri) supports macOS and Linux as well, so porting is possible if there's interest. If you'd like to see a macOS or Linux version, [open an issue](https://github.com/chrisl8/trip-viewer/issues) and let me know.

## Built with AI

This project was built with significant help from [Claude Code](https://claude.ai/claude-code) (Anthropic's AI coding assistant). I'm a full-time software developer, and Claude Code was an excellent collaborator — it helped with architecture decisions, wrote the Rust backend and React frontend, reverse-engineered the dashcam GPS format, and built the entire SD card import pipeline. The result is a codebase I understand fully and maintain myself, with AI as a force multiplier.

If you're curious about how it was built, the [DESIGN.md](DESIGN.md) document covers the architecture decisions and tech stack in detail.

## Feature requests and bug reports

I actively maintain this project and I'm interested in making it better. If you:

- **Found a bug** — [open an issue](https://github.com/chrisl8/trip-viewer/issues) with what happened and what you expected
- **Want a feature** — [open an issue](https://github.com/chrisl8/trip-viewer/issues) describing what you'd like. Some ideas I'm already thinking about: audio source selection, clip export, GPX track export, camera view flipping, and AI-powered footage search
- **Have a different dashcam** — I'd love to add support for it. Open an issue with your dashcam model and, if possible, a sample file

## Development

If you want to build Trip Viewer from source or contribute:

### Prerequisites

- Node.js 20+
- Rust 1.70+ (via [rustup](https://rustup.rs/))
- [HEVC Video Extension](ms-windows-store://pdp/?productid=9N4WGH0Z6VHQ) for HEVC playback

### Build and run

```bash
npm install
npm run tauri dev      # Development mode (hot-reload)
npm run tauri build    # Production build (creates installer)
```

First build compiles the Rust backend (~2 minutes). Subsequent builds use incremental compilation (~10 seconds).

### Tech stack

| Layer | Technology |
|-------|------------|
| App framework | Tauri v2 (Rust backend + WebView2 frontend) |
| Frontend | React 19, TypeScript, Tailwind CSS v4, Zustand |
| Maps | Leaflet + react-leaflet + OpenStreetMap |
| Video sync | `requestVideoFrameCallback` API |
| Container parsing | `mp4` crate (pure Rust, no ffprobe) |
| GPS decoding | Custom ShenShu MetaData binary parser |
| File hashing | SHA-256 via `sha2` crate |
| CI/CD | GitHub Actions + NSIS installer + auto-updater |

See [DESIGN.md](DESIGN.md) for architecture decisions and [RELEASING.md](RELEASING.md) for release instructions.

## License

[MIT](LICENSE)
