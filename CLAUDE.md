# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Development (hot-reload, ~10s incremental Rust builds)
npm run tauri dev

# Production build (creates NSIS installer at src-tauri/target/release/bundle/nsis/)
npm run tauri build

# Type-check TypeScript
npx tsc --noEmit

# Rust tests (35+ unit tests, mostly in src-tauri/src/import/ and src-tauri/src/scan/)
cargo test --manifest-path src-tauri/Cargo.toml

# Run a single Rust test (example)
cargo test --manifest-path src-tauri/Cargo.toml test_copy_and_hash_matches_hash_file

# Clippy (should be zero warnings — enforce this)
cargo clippy --manifest-path src-tauri/Cargo.toml -- -W clippy::all
```

## Architecture

Trip Viewer is a **Tauri v2** desktop app: Rust backend (`src-tauri/src/`) communicates with a React/TypeScript frontend (`src/`) via Tauri commands and events. Targets Windows (NSIS installer) and Linux (AppImage; Flatpak planned). Windows is the primary development platform; the Linux port relies on WebKitGTK 4.1 and GStreamer for video rendering.

### Rust backend module map

- **`scan/`** — folder scanner. Parses Wolf Box filenames (`YYYY_MM_DD_HHMMSS_EE_C.MP4`), fuzzy-matches triplets within a 3-second window, merges segments into trips with a 120s gap threshold. Uses `rayon` for parallel metadata probing.
- **`gps/`** — custom binary parser for the **ShenShu MetaData format** used by Wolf Box firmware. This was reverse-engineered — no upstream spec exists. Decodes NMEA DDMM.MMMM coordinates from the `gpmd` track.
- **`metadata/`** — MP4 probe using the `mp4` crate (pure Rust, **no ffprobe dependency** — this is a locked decision, see DESIGN.md).
- **`import/`** — SD card import pipeline (10 submodules). **Safety-critical**: files are SHA-256 hashed during copy, re-hashed on the destination, and the source is only wiped after every file is verified. See "Import pipeline invariants" below.
- **`error.rs`** — `AppError` enum with `thiserror`; implements `Serialize` for automatic JSON conversion to the frontend.

### Frontend structure

- **`App.tsx`** — sidebar layout with trip list, import button, version footer
- **`components/video/`** — `VideoGrid`, `ChannelPanel`, `PlayerShell`. All three video elements are **always rendered in the same DOM order** (front, interior, rear); swap behavior uses CSS grid placement only. Moving them in the tree would cause React to unmount/remount the `<video>` elements and pause playback.
- **`components/import/`** — confirm dialog, progress UI, unknown files dialog, summary. Progress events stream from Rust via `window.emit()`, frontend listens with `@tauri-apps/api/event`.
- **`engine/useSyncEngine.ts`** — video sync engine. Uses `requestVideoFrameCallback` to track the front channel as the master clock; interior/rear are slaved to it. Front ref is the timing master regardless of which channel is visually primary.
- **`state/store.ts`** — Zustand store with `LibrarySlice`, `PlaybackSlice`, `ImportSlice`. `primaryChannel` controls layout but not sync.

## Locked architectural decisions (do not revisit without strong reason)

See DESIGN.md for full context. Key ones:

- **HTML5 `<video>` for playback** (not libmpv). `tauri-plugin-libmpv` is broken for multi-instance on Windows.
- **Pure Rust `mp4` crate** (not ffprobe). Bundling ffprobe adds 80 MB and triggers Defender heuristics.
- **HEVC Extension tax accepted** — app uses a `<HevcSupportGate>` startup check with Store deep-link on Windows, and an apt-install hint on Linux when GStreamer's libav plugin is missing.
- **NSIS on Windows, AppImage on Linux (Flatpak planned).** MSI rejected (~130 MB vs 3 MB NSIS). `.deb` skipped — AppImage bundles its own GStreamer plugins for codec-complete direct downloads. A future Flatpak would reach Debian/Ubuntu/Fedora/Arch with bundled codecs via `org.freedesktop.Platform.ffmpeg-full`, but no Flathub manifest exists yet.
- **No fullscreen API on single-click** — use double-click (conflict with play/pause expectation).

## Import pipeline invariants

The SD card import pipeline in `src-tauri/src/import/` has strict safety guarantees. Do not break these:

1. **Verify-before-wipe**: `wipe_source()` only runs if `manifest.iter().all(|e| e.verified)` is true, not cancelled, and not read-only.
2. **Cancel safety**: cancel flag is checked between every file operation. Cancel during staging → source NOT wiped.
3. **Lock file with PID recovery**: `<root>/.staging/.lock` contains the PID. On startup, if the PID is dead (verified via Windows `OpenProcess`), the stale lock is reclaimed.
4. **Hash-while-copy**: single-pass SHA-256 via explicit loop (no `TeeReader` in Rust std). Destination is re-hashed independently to detect storage corruption.
5. **Sequential phases**: pre-flight → stage → wipe → distribute → unknowns → cleanup. Each source is processed fully before the next.
6. **PreAllocFiles are skipped during staging** (not just deleted after) — they'd inflate progress counters and waste copy time.
7. **Import root adjustment**: if the user-supplied `root_path` ends in `/Videos`, the parent is used as the import root. Videos/ and Photos/ are siblings at the root.

## Video layout rules

1. All three `<ChannelPanel>` components are always rendered — hidden via CSS grid placement, not conditional rendering.
2. Refs (`frontRef`, `interiorRef`, `rearRef`) are **stable per channel kind**, never swapped. The sync engine depends on this.
3. Audio follows `isMaster` prop (which tracks `primaryChannel`); sync timing is always driven by `frontRef`.
4. On trip/segment change, `primaryChannel` resets to `"front"` in the store action.

## Event system (Rust → frontend)

The import pipeline emits 5 event types via `app.emit()`. Progress events are **throttled to ~15/sec** (66ms minimum between emits) to avoid IPC saturation. See `types.rs` for payload shapes.

## Related documents

- **DESIGN.md** — architecture decisions, ruled-out options, tech stack, future roadmap
- **RELEASING.md** — how to cut a release, version bumping, GitHub Actions workflow, SignPath code signing roadmap
- **README.md** — user-facing documentation
