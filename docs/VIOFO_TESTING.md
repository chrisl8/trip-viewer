# VIOFO playback and GPS checks

This guide tests the VIOFO contribution in PR #21, including its playback and
GPS reliability changes. It does not test the complete Marcus-P fork or its
additional settings, overlay, bookmark and encoder features.

The main archive database remains at schema **13**. Extra analysis and restart
checkpoints are stored beside it in `.tripviewer/analysis-cache-v1.sqlite`.
This file is persistent and reusable on another computer with the archive;
it is not an OS temporary cache. An older application can ignore it. No
migration beyond 13 or modification of original videos is required.

## Source and isolation

Install Git, Node.js 22 LTS and stable Rust. The native prerequisites below
must be installed before building. In a terminal:

```sh
git clone --single-branch --branch contrib/viofo-camera-only https://github.com/Marcus-P/trip-viewer.git trip-viewer-pr21
cd trip-viewer-pr21
git rev-parse HEAD
npm ci
npm run tauri -- build --no-bundle --config src-tauri/tauri.pr21-test.conf.json
```

Keep the printed commit hash with the result: the PR branch may change later.
The override uses the separate application ID `com.tripviewer.pr21check`,
separate application settings, a test window title and disabled updater
endpoints. It does not install over an existing Trip Viewer. Building without
an installer avoids release signing requirements.

Use a **separate test archive** containing copies of a few complete camera sets.
Keep ordinary and protected `RO` subdirectories in their original relative
layout. Do not copy a production `.tripviewer` directory into the test archive:
a full-fork schema newer than 13 is deliberately not opened by this build.
Do not select a production archive just to check playback.

### Ubuntu 24.04 x86_64

Install the native build and playback dependencies:

```sh
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev patchelf gstreamer1.0-libav gstreamer1.0-gl gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly
```

Run the build command above, then launch:

```sh
./src-tauri/target/release/tripviewer
```

The GL plugin is required by WebKit's accelerated rendering path. The AppImage
build includes the GStreamer framework and plugins from its build host. Hardware
VA-API decoding still needs a matching driver on the computer running it.
The existing host-libva compatibility handling is retained. Dependency presence
is not a claim that every GPU, codec or driver has been physically validated.

For a local AppImage rather than a native executable:

```sh
npm run tauri -- build --bundles appimage --config src-tauri/tauri.pr21-test.conf.json
```

The output is under `src-tauri/target/release/bundle/appimage/`. The separate
manually published Ubuntu acceptance packages also provide `start-test.sh` and
an explicit build manifest. They are prereleases without stable updater
metadata; their packaged application ID is `com.tripviewer.pr21test`. Follow
the README included with the selected package, rather than installing the full
fork's Latest Release as a substitute for this contribution.

### Windows 10/11 x64

Install Visual Studio 2022 Build Tools with **Desktop development with C++**,
the MSVC x64/x86 tools and a Windows SDK. Install Microsoft Edge WebView2 Runtime
if it is not already available. Install stable Rust with the MSVC host toolchain,
Node.js 22 LTS and Git, then reopen PowerShell so their PATH changes are loaded.
Run the common clone/build commands above in PowerShell, then:

```powershell
.\src-tauri\target\release\tripviewer.exe
```

WebView2 must be able to decode the recording's codec. HEVC availability depends
on the Windows setup; a successful build alone does not establish HEVC playback.
Do not install extra codecs unless the recording/setup actually needs them.

## Small optional real-recording check

A couple of complete camera sets are useful; a large archive is not required.
Use **Original** at **x1** for the audio test. Check:

1. Play, pause and seek. Camera positions should agree after each action.
2. Double-click the enlarged camera to enter viewer fullscreen. Pause/play
   there, exit by double-click or Escape, and check alignment again. All media
   elements should remain mounted and the shared transport should control them.
3. Select a trip and leave video paused while GPS prepares. The map and speed
   data should appear progressively; completed extraction with no usable points
   and actual failures must not remain labelled as endless loading.
4. If protected recordings are available, check `RO` event highlighting. Parking
   recordings are optional; do not create them solely for this check.

A short result can contain the commit/package, OS, camera model, codec if known,
and which steps worked or failed. No raw videos, GPS coordinates or route data
need to be shared. Local logs may contain private paths; do not post them without
checking their contents. Reports for one camera/setup do not validate others.

The owner confirmed GPS tracks, merged-trip tracks and the moving marker in the
isolated Ubuntu GPS candidate. That earlier test still had playback stutter and
fullscreen/synchronization failures; it is not playback acceptance of subsequent
revisions. The A129 result must be reported separately by its tester.

## Automated checks

```sh
npm test
npx tsc --noEmit
cargo test --manifest-path src-tauri/Cargo.toml --lib --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --lib --tests --locked -- -D warnings
```

These checks cover code-level behavior and archive compatibility. They do not
replace real WebKit/WebView2 playback or camera/GPU tests. Codex assists with
preparation; hands-on tests and contributor findings retain their attribution.
