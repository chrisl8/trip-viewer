# DashCam Viewer — Open Source Design Document

## Project Name (TBD)

Working name ideas: `roadlog`, `dashview`, `tripviewer`, `reelroad`

---

## Problem Statement

Existing dashcam viewing software falls into two camps:

1. **Manufacturer apps** (Wolf Box, Thinkware, Viofo) — auto-detect their own multi-channel files and show them simultaneously with GPS, but have terrible UX: buried speed controls, no scrubbing, poor performance (software video decoding), and no interoperability.
2. **Third-party viewers** (Dashcam Viewer, DVPlayer, bbplay) — better UX and performance, but struggle with multi-channel support due to inconsistent file naming across manufacturers, proprietary GPS encodings, and a max of 2 simultaneous channels.

There is **no open-source, multi-channel, GPS-aware dashcam viewer** that uses hardware-accelerated video decoding.

---

## Feature Inventory

### Features from existing software (what people expect)

| Feature                        | Wolf Box    | DCV                 | DVPlayer   | bbplay    |
| ------------------------------ | ----------- | ------------------- | ---------- | --------- |
| Multi-channel sync playback    | ✅ (3ch)    | ✅ (2ch PiP)        | ✅ (2-4ch) | ✅ (n-ch) |
| Live GPS map                   | ✅          | ✅                  | ✅         | ✅        |
| Speed/heading display          | ✅          | ✅                  | ✅         | ✅        |
| G-force/accelerometer graphs   | ❌          | ✅                  | ❌         | ✅        |
| Speed over time graph          | ❌          | ✅ (clickable)      | ❌         | ✅        |
| Variable playback speed        | ✅ (buried) | ✅                  | ✅         | ✅        |
| Timeline scrubbing             | ❌          | ✅                  | ✅         | ✅        |
| Folder/batch loading           | ❌          | ✅ (trip detection) | ✅         | ✅        |
| Trip segmentation              | ❌          | ✅                  | ❌         | ❌        |
| Geotagging / markers           | ❌          | ✅ (Plus/Pro)       | ❌         | ❌        |
| Clip export                    | ❌          | ✅                  | ✅         | ❌        |
| GPS track export (GPX/KML/CSV) | ❌          | ✅                  | ❌         | ❌        |
| Audio event detection          | ❌          | ✅                  | ❌         | ❌        |
| Snapshot capture               | ❌          | ✅                  | ✅         | ❌        |
| Hardware-accelerated decoding  | ❌          | ✅                  | ✅         | ?         |

### Features Chris specifically wants

1. **All 3 camera views simultaneously** (front, interior, rear)
2. **Live map showing vehicle position** as video plays
3. **Easy speed control** — prominent, not buried
4. **Scrub forward/back** — drag timeline
5. **Load a folder** of files seamlessly
6. **Smooth playback** — hardware-accelerated decoding

### Future / stretch features to design for

1. **Automated footage analysis** — scene change detection, object detection, interesting moment flagging
2. **OCR on frames** — extract speed limit signs, text overlays
3. **Batch GPS export** — process entire SD card → GPX/KML tracks
4. **Trip journal / map** — all trips plotted on a map, click to jump to footage
5. **AI-powered search** — "find the clip where I passed the red barn" (local vision model)
6. **Plugin/extension system** — camera-specific GPS parsers as plugins
7. **Timelapse generation** — from full trip footage
8. **Incident bookmarking** — flag moments for later review
9. **Speed overlay** — bake GPS speed into exported clips
10. **OpenStreetMap contribution pipeline** — extract GPS tracks + frames for mappers

### Ideas after MVP that I thought of (may already be listed, but maybe not and are probably more important than the "Future" section)

- Ability to see what view the audio is coming from and select a different view to be the audio source.
- Ability to flip a camera's view.
  - Also save that on restarts.
- Save last folder and open it again on restart.
- Ability to click a "side" video and have it replace the "main" video as the main, in other words, clicking on a side video rearranges them to make it the "big one"
- Ability to click on the "main" video and have it fill the ENTIRE screen and go back again for a closer look.
- Some system to review error files, see what is wrong with them and delete them if desired from the interface.

---

## Architecture Decision: How to Build This

### Option A: Web-based (Electron/Tauri + HTML5 Video)

**Pros:**

- You know React/Node.js deeply
- Leaflet/MapLibre for maps is trivial
- Beautiful UI with full CSS control
- Tauri is lightweight (~5MB vs Electron's ~100MB)
- Web `<video>` elements use hardware decoding natively

**Cons:**

- HTML5 `<video>` has limited codec support (no HEVC without paid license on some platforms)
- Synchronizing 3 `<video>` elements is doable but imprecise at frame level
- Tauri's libmpv plugin exists but is experimental on Linux

**Verdict:** ⭐ **Strong candidate** — especially a Tauri app with React frontend

### Option B: mpv-based (Python/Node wrapper around libmpv)

**Pros:**

- mpv handles ALL codecs with hardware acceleration
- `python-mpv` or `node-mpv` libraries exist
- mpv's `--lavfi-complex` can composite multiple streams into one output
- IPC (JSON socket) allows full external control
- Lua scripting built in for extensions

**Cons:**

- Synchronizing multiple mpv instances is tricky (documented in GitHub issues)
- Embedding mpv in a GUI framework (for the map/graph panels) is non-trivial
- UI is either mpv's OSD (limited) or you need a separate GUI framework alongside it

**Verdict:** Great for the video engine, but needs a UI layer on top

### Option C: Hybrid — Tauri + libmpv for video, web for everything else

**Pros:**

- Best of both worlds: mpv handles video (hardware accel, any codec), web handles UI
- `tauri-plugin-libmpv` exists specifically for this
- React frontend for map (Leaflet), graphs (Recharts/D3), controls
- Rust backend in Tauri for file scanning, GPS extraction, metadata parsing
- Small binary size

**Cons:**

- Tauri libmpv plugin is "experimental" on Linux (your primary platform)
- More moving parts
- Need to coordinate between mpv rendering and web overlay

**Verdict:** ⭐⭐ **Ideal long-term architecture**, but higher initial complexity

### Option D: Pure Python (PyQt/PySide + mpv + Folium/QtWebEngine for maps)

**Pros:**

- Python ecosystem is great for data processing, GPS parsing, CV
- PyQt6 + mpv widget embedding is well-documented
- Can use ffmpeg-python for metadata extraction
- Prototyping speed is fast

**Cons:**

- Distribution/packaging is painful (PyInstaller, etc.)
- UI aesthetics harder to nail vs web tech
- You're less fluent in Qt than React

**Verdict:** Good for prototyping, not ideal for a polished tool

### ⭐ Recommended: Start with Option A (Tauri + React), graduate to C

Start simple:

1. **Phase 1:** Tauri app, 3x HTML5 `<video>` elements, Leaflet map, custom controls. This gets you 80% of the way with tech you already know. Test with your Wolf Box files.
2. **Phase 2:** If HTML5 video has codec issues or sync problems, swap in mpv via the Tauri plugin or a sidecar process controlled over IPC.
3. **Phase 3:** Add the analysis pipeline (Python sidecar for CV/AI tasks).

---

## Key Libraries & Tools

### Video playback

- **HTML5 `<video>`** — hardware-accelerated, works for H.264 MP4 (most dashcams)
- **mpv** (fallback) — universal codec support, `tauri-plugin-libmpv` or IPC control
- **ffprobe / ffmpeg** — metadata extraction, clip export, transcoding

### GPS extraction

- **exiftool** — reads GPS from most MP4 metadata formats
- **ffprobe** — can detect GPS data streams in MP4 containers
- **nvtk_mp42gpx** (Python) — extracts GPS from Novatek chipset cameras
- **piofo** (Python) — extracts GPS from Viofo cameras specifically
- **gopro2gpx** (Python) — extracts GPS from GoPro's GPMD format
- **Custom parser needed** — Wolf Box likely uses its own GPS encoding; will need to reverse-engineer from your files

### Maps

- **Leaflet.js** + OpenStreetMap tiles — free, self-hostable, great React bindings (`react-leaflet`)
- **MapLibre GL JS** — alternative if you want vector tiles / 3D

### UI framework

- **Tauri v2** — Rust backend, web frontend, tiny binaries
- **React** — your bread and butter
- **Recharts or D3** — for speed/g-force/altitude graphs
- **Tailwind CSS** — rapid styling

### Analysis pipeline (Phase 3+)

- **ffmpeg scene detection** — `select='gt(scene,0.3)'` for keyframe extraction
- **YOLO / Ultralytics** — object detection (vehicles, people, signs)
- **Tesseract / EasyOCR** — text extraction from frames (speed limit signs)
- **OpenCV** — motion detection, frame differencing
- **Local LLM vision** — describe scenes for search (llava, etc.)

---

## Data Model

```
Trip
├── id: uuid
├── start_time: datetime
├── end_time: datetime
├── segments: Segment[]
│
Segment (one continuous recording period)
├── id: uuid
├── start_time: datetime
├── duration: seconds
├── channels: Channel[]
│   ├── type: "front" | "rear" | "interior"
│   ├── file_path: string
│   ├── resolution: string
│   ├── fps: number
│   └── codec: string
├── gps_track: GpsPoint[]
│   ├── timestamp: datetime
│   ├── lat: float
│   ├── lon: float
│   ├── speed_kmh: float
│   ├── heading: float
│   └── altitude: float
├── events: Event[] (bookmarks, detected incidents)
│   ├── timestamp: datetime
│   ├── type: "bookmark" | "audio_spike" | "hard_brake" | "scene_change"
│   └── metadata: json
```

---

## File Detection Strategy

Since every dashcam manufacturer does naming differently, we need a flexible detection system:

```
Input: A folder path
Output: A list of Trips, each containing Segments with matched Channels

Algorithm:
1. Scan folder recursively for video files (.mp4, .mov, .ts, .avi)
2. For each file, run ffprobe to get:
   - Duration, resolution, codec, fps
   - Any embedded GPS data streams
   - Creation timestamp
3. Group files by base filename (strip _F, _R, _I, _A, _B, _C, _D suffixes)
4. Within each group, assign channel types by suffix
5. Sort groups by timestamp
6. Merge consecutive groups (gap < configurable threshold) into Trips
7. Extract GPS tracks (from embedded data, sidecar files, or subtitle streams)

Camera-specific parsers registered as plugins:
- WolfBoxParser: _F/_R/_I naming, specific GPS format
- ViofoParser: _F/_R naming, Novatek GPS
- BlackVueParser: _F/_R naming, subtitle-stream GPS
- GenericParser: fallback, tries common patterns
```

---

## UI Layout (Phase 1)

```
┌──────────────────────────────────────────────────────┐
│  [≡ Menu]          Trip: 2026-04-08 Morning Commute  │
├──────────────┬──────────────┬────────────────────────┤
│              │              │                         │
│   FRONT      │   INTERIOR   │      MAP               │
│   (large)    │   (medium)   │   (Leaflet, live       │
│              │              │    vehicle marker,      │
│              │              │    trail behind)        │
│              ├──────────────┤                         │
│              │    REAR      │   Speed: 45 mph        │
│              │   (medium)   │   Heading: NE           │
│              │              │                         │
├──────────────┴──────────────┴────────────────────────┤
│  ◀◀  ▶  ▶▶  │  0.5x  1x  2x  4x  8x  │  Vol 🔊    │
├──────────────────────────────────────────────────────┤
│  ████████████████░░░░░░░░░░░░░░░░░░░░░  12:34/45:00 │
│  ▲ speed graph overlaid on timeline (clickable)      │
├──────────────────────────────────────────────────────┤
│  Trip segments: [seg1] [seg2] [seg3] [seg4] ...      │
└──────────────────────────────────────────────────────┘
```

Key UX decisions:

- **Front camera gets the most screen real estate** (primary view)
- **Speed control is a prominent row of buttons**, not a menu
- **Timeline is a large scrub bar** with speed graph overlay so you can see interesting moments
- **Map updates in real-time** as video plays, with a trail showing where you've been
- **Trip segments** shown as a filmstrip at the bottom for easy navigation
- **Keyboard shortcuts**: Space=play/pause, ←→=seek 5s, Shift+←→=seek 30s, [/]=speed

---

## Phase Plan

### Phase 1: MVP (get your own footage playing nicely)

- [ ] Tauri app scaffolding with React
- [ ] Folder scanner: detect Wolf Box file naming, group into segments
- [ ] 3x `<video>` synchronized playback with shared controls
- [ ] ffprobe-based metadata extraction (Rust sidecar or CLI call)
- [ ] GPS extraction from your specific Wolf Box format
- [ ] Leaflet map with moving marker
- [ ] Timeline scrubber, speed controls, keyboard shortcuts
- [ ] Basic trip detection (group files by time gaps)

### Phase 2: Polish & generalize

- [ ] Plugin system for camera-specific parsers
- [ ] Speed/altitude/g-force graphs (if data available)
- [ ] Clip export (selected time range → new MP4)
- [ ] Snapshot capture
- [ ] GPX/KML export
- [ ] Bookmarking / event markers on timeline
- [ ] Settings: preferred map tile source, units (mph/kmh), default speed

### Phase 3: Analysis & automation

- [ ] Batch GPS extraction (entire SD card → trip map)
- [ ] Scene change detection → thumbnail timeline
- [ ] Audio spike detection (horn, crash sounds)
- [ ] Object detection pipeline (YOLO on keyframes)
- [ ] OCR for text in frames (signs, plates)
- [ ] Search by location ("clips near downtown Wichita")

### Phase 4: Community & ecosystem

- [ ] Public GitHub repo, documentation
- [ ] Camera compatibility database (community-contributed parsers)
- [ ] Timelapse generator
- [ ] OpenStreetMap integration (contribute GPS traces)
- [ ] Local AI-powered clip search

---

## Open Questions

1. **What GPS format does the Wolf Box use?** Need to `ffprobe` an actual file to see if GPS is in a subtitle stream, data stream, or proprietary atom.
2. **Are the 3 Wolf Box channels separate files or muxed?** The `_F/_I/_R` naming suggests separate files, which is easier.
3. **What resolution/codec/fps are the files?** Determines if HTML5 `<video>` can handle them without transcoding.
4. **Cross-platform priority?** Linux-first (your daily driver), but Tauri gives us Windows/Mac for free.
5. **Project name and license?** MIT? GPL? Something fun for the name?

---

## Next Step

**Run `ffprobe -show_streams -show_format` on one of your Wolf Box files** (one from each channel). This tells us everything we need to know about codec, GPS data location, and file structure to make the first real architecture decisions.
