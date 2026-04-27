# Linux dev setup

This document covers how to get a working Trip Viewer dev environment on Linux, with two paths depending on your distro:

- **[Atomic / immutable distros](#atomic--immutable-distros-bazzite-silverblue-kinoite)** (Bazzite, Silverblue, Kinoite, etc.) — use distrobox so you don't have to layer packages onto the base image.
- **[Mutable distros](#mutable-distros-vanilla-fedora-ubuntu-arch)** (vanilla Fedora workstation, Ubuntu, Arch, etc.) — install the deps directly on the host.

The end state is the same in both cases: `npm run tauri dev` runs and HEVC video plays. The runtime AppImage produced by CI bundles its own WebKitGTK + GStreamer codec stack and needs no host setup beyond `libfuse2` (which Fedora and most other distros ship by default).

---

## Atomic / immutable distros (Bazzite, Silverblue, Kinoite)

These distros use `rpm-ostree` and discourage layering packages onto the base image. The clean path is a Fedora toolbox container via [distrobox](https://distrobox.it/). Distrobox is preinstalled on Bazzite; on Silverblue/Kinoite install it with `rpm-ostree install distrobox` (one-time, requires reboot).

### 1. Create the container

```bash
distrobox create --name tripviewer-dev \
  --image registry.fedoraproject.org/fedora-toolbox:43 --yes
```

Match the Fedora version to your host's base image (`rpm -E %fedora` on the host tells you).

### 2. Install Tauri build dependencies

```bash
distrobox enter tripviewer-dev -- sudo dnf install -y \
  webkit2gtk4.1-devel \
  gtk3-devel \
  libsoup3-devel \
  javascriptcoregtk4.1-devel \
  librsvg2-devel \
  openssl-devel \
  atk-devel \
  gcc gcc-c++ pkg-config
```

After this, `cargo build --manifest-path src-tauri/Cargo.toml` will succeed inside the container.

### 3. Enable RPM Fusion (required for codec packages)

```bash
distrobox enter tripviewer-dev -- sudo dnf install -y \
  https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm \
  https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm
```

`rpm -E %fedora` runs inside the container, so it picks up the container's Fedora version automatically.

### 4. Install the GStreamer codec stack (HEVC playback)

```bash
distrobox enter tripviewer-dev -- sudo dnf install -y \
  gstreamer1-plugin-libav \
  gstreamer1-plugins-ugly \
  gstreamer1-plugins-bad-freeworld \
  libavcodec-freeworld
```

Why each:

- **`gstreamer1-plugin-libav`** — ffmpeg-backed decoder shim for GStreamer.
- **`libavcodec-freeworld`** — RPM Fusion's unstripped libavcodec. Required because Fedora's `gstreamer1-plugin-libav` links against `libavcodec-free`, which has HEVC stripped. Installing freeworld puts an unstripped lib at `/usr/lib64/ffmpeg/libavcodec.so.61` that takes precedence.
- **`gstreamer1-plugins-ugly`** + **`gstreamer1-plugins-bad-freeworld`** — additional codec coverage matching what the CI release workflow installs on Ubuntu.

### 5. Clear the stale GStreamer registry cache

```bash
distrobox enter tripviewer-dev -- rm -rf ~/.cache/gstreamer-1.0
```

**Don't skip this.** GStreamer caches its plugin registry per-user; without clearing it, `gst-inspect-1.0 libav | grep h265` will report nothing and the app will fall back to non-HEVC paths. The cache regenerates automatically on next use.

### 6. Verify

```bash
distrobox enter tripviewer-dev -- bash -c '
  cd /path/to/trip-viewer &&
  cargo build --manifest-path src-tauri/Cargo.toml &&
  gst-inspect-1.0 libav | grep -i h265
'
```

Expect: cargo build succeeds, and `avdec_h265: libav HEVC ... decoder` appears.

> **`npm run tauri build` (AppImage bundling) is currently not viable on Fedora derivatives.** Tauri's bundler downloads `linuxdeploy-plugin-gstreamer` but never invokes it, so no GStreamer plugin `.so` files end up bundled. The resulting AppImage's bundled libgstreamer doesn't reliably load host plugins on Bazzite (Fedora 43) even with `GST_PLUGIN_SYSTEM_PATH_1_0` and friends set, leading to `appsink not found` / `autoaudiosink not found` and a frozen player on launch. Use `npm run tauri dev` (next section) for actual development and testing on Linux. A proper bundling fix is a v0.3+ workstream — see the runtime AppImage section at the bottom of this doc.

### Daily use

```bash
distrobox enter tripviewer-dev
cd /path/to/trip-viewer
npm run tauri dev
```

The host's cargo (`~/.cargo`), fnm-managed node, and `~/.config` carry through automatically — distrobox auto-mounts `/home`, `/run/user`, `/run/media`, and `/mnt`. Git/SSH/editor configs work the same as on the host.

---

## Mutable distros (vanilla Fedora, Ubuntu, Arch)

No distrobox needed — just install equivalents of the same packages on the host.

### Fedora workstation

```bash
# Build deps
sudo dnf install -y \
  webkit2gtk4.1-devel gtk3-devel libsoup3-devel javascriptcoregtk4.1-devel \
  librsvg2-devel openssl-devel atk-devel gcc gcc-c++ pkg-config

# RPM Fusion (one-time)
sudo dnf install -y \
  https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm \
  https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-$(rpm -E %fedora).noarch.rpm

# Codecs
sudo dnf install -y \
  gstreamer1-plugin-libav gstreamer1-plugins-ugly \
  gstreamer1-plugins-bad-freeworld libavcodec-freeworld

# Clear stale cache
rm -rf ~/.cache/gstreamer-1.0
```

### Ubuntu / Debian

This mirrors what `.github/workflows/release.yml` installs in CI:

```bash
sudo apt install -y \
  libwebkit2gtk-4.1-dev build-essential pkg-config libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev file \
  gstreamer1.0-libav gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly
```

Ubuntu's `gstreamer1.0-libav` is unstripped, so no freeworld equivalent needed.

### Arch

```bash
sudo pacman -S \
  webkit2gtk-4.1 gtk3 libsoup3 librsvg openssl atk \
  base-devel pkgconf \
  gst-libav gst-plugins-good gst-plugins-bad gst-plugins-ugly
```

Arch's `gst-libav` is unstripped.

---

## Runtime AppImage on Linux

Most distros need nothing extra to run the released `.AppImage`. The Tauri AppImage build bundles WebKitGTK 4.1, GTK3, libsoup3, javascriptcoregtk, librsvg2, and the GStreamer codec plugins via `linuxdeploy` at CI time.

The only host requirement is **libfuse2** (to mount the AppImage), which Fedora, Ubuntu, Arch, Bazzite, and most other distros ship by default. If `./TripViewer.AppImage` exits with a "fusermount" error, install your distro's `fuse` (libfuse2) package.

The optional **timelapse** feature shells out to ffmpeg, which is *not* bundled. On Bazzite, ffmpeg from RPM Fusion non-free (with NVENC/NVDEC) ships preinstalled at `/usr/bin/ffmpeg`. On other distros, install ffmpeg however you normally would and point the app's settings at the binary.

### Known issue: v0.2.0 release AppImage on Fedora derivatives

The released `Trip Viewer_0.2.0_amd64.AppImage` from GitHub does **not** work on Fedora 41+, Bazzite, Silverblue, Kinoite, or other distros with GLib 2.80+ / GStreamer 1.26+. The CI build runs on Ubuntu 22.04, which ships GLib 2.72 / GStreamer 1.20; the bundled WebKit can't load the host's newer GStreamer plugins because the bundled libs lack symbols like `g_once_init_leave_pointer` and `gst_id_str_as_str` that newer plugins require. Symptoms include `GStreamer element autoaudiosink not found` and a long cascade of `Failed to load plugin … undefined symbol …` warnings, followed by the player freezing when you try to load a trip.

A locally-built AppImage from the dev container has matching libgstreamer/libglib ABI but still fails for a separate reason (Tauri's bundler does not invoke `linuxdeploy-plugin-gstreamer`, so no GStreamer plugins end up bundled — and the bundled libgstreamer doesn't reliably fall back to host plugins on Fedora). So **there is currently no working AppImage path on Bazzite/Fedora 43**.

**Workaround for daily use on Fedora derivatives**: develop and run from inside the dev container — `npm run tauri dev` for a hot-reload session, or run the compiled release binary directly with `distrobox enter tripviewer-dev -- /path/to/trip-viewer/src-tauri/target/release/tripviewer` after a `npm run tauri build`. (`distrobox-export --bin <path>` will also expose the binary on the host PATH.)

A proper fix — bundling GStreamer plugins via `linuxdeploy-plugin-gstreamer` as a post-build step — is planned for v0.3+.
