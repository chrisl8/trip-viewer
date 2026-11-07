# VIOFO and playback contribution: validation scope

These additions were developed/tested in the Marcus-P fork of chrisl8/trip-viewer.
Original project attribution, icons and licenses are retained. Björn Fries
(`gubiq`) authored the archive-auto-scan and Linux libva/DMABuf fixes; their
commits and co-author notices are preserved in this contribution branch.

## Current follow-up: shared functionality from stable fork 0.9.0

This PR now includes the shared display functionality from published
[fork 0.9.0](https://github.com/Marcus-P/trip-viewer/releases/tag/v0.9.0), source
`a89fe4ceb98865db2a1d30dfc70921f588be095e`. The
[incremental diff since the previous PR head](https://github.com/Marcus-P/trip-viewer/compare/4fc170ae9f4617799fe8456bb48e71d8be82c187...contrib/viofo-playback-081)
isolates this update, preserving all original commits and contributor credit.

- Classic tiled playback remains the default with its existing saved proportions.
  Overlay adds Front as background, a rear-view mirror mask and independently
  movable/resizable camera layers, including Interior and additional labels.
- Frame geometry is separate from the video crop. Horizontal/vertical crop
  controls select the road instead of the sky; zoom/position controls enable
  direct crop dragging. The mask clips the composited video itself, with a frame
  drawn above it and transparent outside corners showing Front.
- Positions, zoom, crop, shape and stacking persist. Reset crop restores 100%
  zoom and centers both axes without moving the frame or another camera;
  Reset overlay restores layer defaults. Save/Cancel preserve draft semantics.
- Dashboard fullscreen retains the composition. Single-camera fullscreen shows
  the full source and restores the crop on exit. Layout changes reuse the same
  video elements and SyncEngine, without replacing sources or changing audio.

For a quick app-level test without compiling, the published fork release above
contains Windows x64, Ubuntu Debian/AppImage and experimental universal Mac
downloads. Those are **fork binaries**, not a new upstream release. The PR keeps
upstream's application version **0.3.3**, dependencies, updater/endpoint, signing,
workflows, icons and license material. Maintainer release decisions remain yours.
Use a copy/backup of an archive: the schema 13-to-17 migration is part of the
original contribution; an unmodified schema-13-only build cannot reopen it.
This display follow-up adds no migration or GPS parser change.

The owner accepted the overlay, crop controls and mirror masking, paused and
playing, with VIOFO A229 Pro 3CH footage on Ubuntu 24.04 LTS, and confirmed the
installed stable 0.9.0 works. The final reset is also checked with rendered video
center alignment, both position sliders, Save/Cancel and restart restoration.
This does not add a Windows/Mac overlay certification or certify other cameras.

The updated contribution passes **110 frontend tests** and the TypeScript/Vite
production build. All fourteen shared frontend files match stable fork 0.9.0;
Settings deliberately omits the fork's release-update widget. Static comparisons
confirm the upstream distribution/update files remain unchanged. No Rust runtime
code changes in this follow-up. The corresponding fork PR/package CI
[42283614546](https://github.com/Marcus-P/trip-viewer/actions/runs/42283614546)
and merged release
[42307839895](https://github.com/Marcus-P/trip-viewer/actions/runs/42307839895)
passed for all three platforms. No duplicate native package pipeline is started
for this port; any upstream workflow approval remains with the maintainer.

The fork maintainer's hands-on camera testing covers **VIOFO A229 Pro 3CH on
Ubuntu 24.04 LTS and Windows 11 x64**. The Ubuntu acceptance covers playback/GPS
preparation, segment transitions, map seeking, dashboard timeline, paused zoom,
bookmarks including restart and channel visibility. On 2026-10-23 the owner also
reported successful Windows 11 operation without installing another codec or
plugin; this does not infer an exhaustive per-feature/hardware matrix.

Windows HEVC availability depends on the existing decoder and drivers. On the
tested Windows 10 computer, the OEM HEVC Store package linked by the app was
unavailable. The general Microsoft HEVC Video Extensions package was offered
for €0.99; the Store's displayed price/availability may vary. **Windows 10 playback
after buying it is untested.** Installing VLC alone does not give WebView2 VLC's
decoder. No new Mac real-footage test is claimed; upstream's Mac packaging and
signing claims remain its own. No other real camera is newly certified.

Existing Wolf Box, Thinkware, Miltona and generic implementations remain. Their
naming/grouping and Wolf Box/Miltona decoder files are unchanged from upstream.
Automated tests pass, and generated H.264 clips in the legacy naming formats
were scanned/decoded/played in native WebKit with 3/2/1/4 channels respectively.
Those synthetic clips contain no proprietary camera GPS tracks; they cannot
confirm real device/firmware/codec behaviour. Lack of confirmation does not mean
those cameras stopped working. Thinkware still has no GPS decoder as before.
A129 models were not tested by the fork maintainer; coordinate with PR #12's work.

Source changes include VIOFO normal/parking recognition and GPS validation,
resumable progressive extraction, bounded reads, persistent camera health,
playback recovery, shared trip-time route/bookmark seeking, paused zoom and saved
camera visibility. Database schema advances to 17; upgraded archives cannot be
opened by schema-13-only upstream builds. GPS parser version is 8 and the
existing speed-curve envelope remains v1.

This branch retains upstream package/application versions, updater, release
workflows, signing configuration and existing documentation rather than applying
the fork's separate distribution and release policy. The upstream maintainer controls the
release/version decision. No original user media is rewritten by these features.

Follow-up consistency fixes from the fork's 0.7.0 work make the empty-library
message camera-neutral and register the existing Miltona diagnostic-export
command used by the UI. They do not change the GPS parser, schema, upstream
release/updater configuration or the real-camera validation scope above.

Checkpoint test fixtures now use the canonical archive root for both recording
paths and media-health keys, matching production scans on Windows/macOS. The
relative checkpoint path is checked after JSON decoding. All four checkpoint
tests pass with an aliased temporary root, which reproduced the three original
fixture failures before the correction. Production parser/cache behaviour and
upstream distribution configuration are unchanged by this test-only follow-up.


## Earlier settings update: fork 0.8.1

This contribution uses upstream main `a9617fcb0ec53bd23a627fa7f5c30cbd3a65d148`
and incorporates the shared application changes from fork release
[0.8.1](https://github.com/Marcus-P/trip-viewer/releases/tag/v0.8.1), source
`97e1858262b6c255ad637c1116455358f4312d63`. The contribution keeps upstream's
own application/package version; the fork's release number is a source reference.

The GPS parser, player synchronization, archive schema and checkpoint fixes match
that fork release. The latest follow-up groups camera visibility and Show map in
**Settings → Display layout**, replacing the row above the player. Front,
Interior, Rear and additional archive channel labels are available. Save applies
the draft to the mounted player; Cancel discards it. The existing layout key and
saved selections are retained. Small-window settings/navigation can scroll.

Upstream's updater, update endpoint, signing configuration, package dependencies,
release workflows and original icons/license material are retained. The fork's
installer/release/update implementation is separate from these shared changes.

## Review map

| Area | Main entry points |
| --- | --- |
| VIOFO recognition and GPS | `scan/viofo.rs`, `gps/viofo.rs`, `gps/viofo_timing.rs`, `gps/validation.rs`, `gps/mod.rs` |
| Progressive GPS persistence and camera health | DB migrations 0014–0016, `db/gps_segment_checkpoints.rs`, `media_health.rs`, `state/gpsPreparation.ts` |
| Shared playback/recovery | `engine/SyncEngine.ts`, `components/video/PlayerShell.tsx`, `VideoGrid.tsx`, `mediaRecovery.ts` |
| Map seeking and bookmarks | `components/map/routeSeek.ts`, `components/timeline/useBookmarks.ts`, `bookmarks.rs`, DB migration 0017 |
| Latest settings follow-up | `components/settings/SettingsDialog.tsx`, `settings/layout.ts`, its test, `PlayerShell.tsx`, `MainNavTabs.tsx` |
| Overlay/crop follow-up from 0.9.0 | `components/video/VideoLayoutControls.tsx`, `OverlayHandles.tsx`, `useCameraLayout.ts`, `useOverlayCrop.ts`, `overlayMask.ts`, `settings/overlayLayout.ts` and their tests |

Frontend paths above are relative to `src/`; Rust paths are relative to
`src-tauri/src/`. Original contributor commits remain intact, with the settings
and evidence update added as a separate follow-up for review.

The original thirteen migration scripts are unchanged from upstream. A separate
in-memory SQL smoke upgraded an upstream schema-13 fixture through migrations
14–17, preserving its trip and GPS rows and passing integrity/foreign-key checks.
This verifies the SQL upgrade path, not every real archive or the old binary's
ability to reopen an upgraded archive. Back up archives before testing.

Display-settings follow-up validation: TypeScript typecheck and Vite production build pass;
ten focused tests cover saved-layout notifications/persistence, camera placement
and map visibility. Static comparisons confirm that shared GPS/sync/database
code matches fork 0.8.1 and upstream's versions, dependency locks, updater,
signing/release scripts, workflows, icons and license files are retained.
The follow-up changes no Rust runtime code or migration scripts.


## Archive-switch bugfix (released in fork 0.8.2)

The contribution also includes the shared bugfix from
[fork PR #10](https://github.com/Marcus-P/trip-viewer/pull/10), published in
[fork 0.8.2](https://github.com/Marcus-P/trip-viewer/releases/tag/v0.8.2) and retained
in 0.9.0. The fork's release/version metadata is not copied into this PR.

- Selecting another archive, or reselecting the same folder, automatically
  reloads the recording list. Old playback/library/cache state is cleared.
- Native archive opens are serialized. Delayed startup, scan, GPS and metadata
  replies cannot overwrite a newer selection. Import completion requests a
  fresh scan after any older in-flight scan, so copied recordings are included.
- The last reachable archive is restored on startup. Archive and import-source
  picker locations are remembered separately; cancelling preserves the choice.
- The backend saves settings before replacing its active archive slot. Failed
  saves retain prior in-memory settings. Queued scans of unrelated previous
  roots are rejected before they can persist or garbage-collect database rows.

Review entry points: `src/state/archiveNavigation.ts` and its tests,
`src/components/loader/TripLoader.tsx`, `src/ipc/dialog.ts`, `src/state/store.ts`,
`src/state/gpsPreparation.ts`, `src-tauri/src/archive.rs`,
`src-tauri/src/app_settings.rs` and `src-tauri/src/scan/mod.rs`.

This updated contribution passes **100 frontend tests** and the TypeScript/Vite
production build. Native regression cases cover remembered-folder persistence,
failed settings writes and scan-root isolation; native/package results are
recorded in fork PR #10. On 2026-10-25 the owner confirmed the requested
A → B → restart test, including automatic recording refresh, restoring B and
starting the archive picker there. This acceptance preserves the camera/OS
scope above; no broader platform matrix is inferred.
The fork version files and release metadata are separate from this bugfix commit
and are not part of this contribution. Upstream's updater/signing/versioning
remain intact. No new DB migration, parser version or original-media rewrite.
