# Releasing Trip Viewer

Pushing a version tag (e.g. `v0.2.0`) to the GitHub mirror triggers a GitHub Actions workflow that builds a Windows NSIS installer and a Linux AppImage, signs both for the auto-updater, and creates a draft release with the installers and update manifest attached. You review the draft, edit the release notes if desired, and publish it.

> **Flatpak:** a Flathub distribution is planned but **not part of this release workflow**. There is no Flathub manifest repo yet; AppImage is the only Linux artifact the CI produces. When Flatpak is added, it will ship out-of-band via Flathub (which manages its own updates), not from this repo's GitHub Actions.

## One-time setup

These steps only need to be done once.

### 1. Signing keys

The Tauri updater requires a key pair. The private key signs each release; the public key is embedded in the app so it can verify updates.

- **Private key**: `~/.tauri/trip-viewer.key`
- **Public key**: `~/.tauri/trip-viewer.key.pub` (also in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`)

**If you lose the private key, existing installs will never be able to receive updates.** Back it up.

To regenerate (only if needed — this invalidates all existing installs):

```bash
npx tauri signer generate -w ~/.tauri/trip-viewer.key
```

Then update the `pubkey` in `src-tauri/tauri.conf.json` and the GitHub secrets.

### 2. GitHub secrets

Go to [github.com/chrisl8/trip-viewer](https://github.com/chrisl8/trip-viewer) > Settings > Secrets and variables > Actions, and set:

| Secret                               | Value                                          |
| ------------------------------------ | ---------------------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | Contents of `~/.tauri/trip-viewer.key`         |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The password you chose when generating the key |

`GITHUB_TOKEN` is provided automatically by GitHub Actions.

### 3. Rotating the updater signing key

**Rotation invalidates auto-update for every existing install.** A shipped binary embeds one pubkey forever; the new key's signatures cannot be verified by anything built with the old pubkey. So only rotate when you must (lost key, compromised key, or cleaning up a broken deployment like v0.1.14 — see commit `4e97198`), and plan the rollout:

1. Generate the new keypair with `npx tauri signer generate -w ~/.tauri/trip-viewer.key -f`.
2. Replace the `pubkey` in `src-tauri/tauri.conf.json` with the contents of the new `.key.pub`.
3. Update both GitHub secrets (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) **before** pushing the config change.
4. Before tagging the first release with the new key, run `bash scripts/check-signing-key.sh` locally (or rely on the release-workflow preflight) to confirm key and config agree. The preflight fails the release job in seconds if they don't.
5. The first release after rotation must include a manual-reinstall callout in its GitHub Release notes — existing installs cannot auto-update across the rotation. Suggested wording:
   > **One-time manual update required if you are on an older build.** The auto-updater signing key was rotated; your current install can't verify new signatures. Download the installer below and run it once; auto-updates will work normally from that point on.

## Before tagging: pre-flight check

Two checks run automatically on every push to `main` and every PR, in the `Verify updater (dry run)` workflow:

- Signs and verifies a throwaway stub artifact end-to-end via `scripts/verify-updater.sh`.
- Exercises `scripts/check-signing-key.sh` against a throwaway tauri-generated keypair — the same script the release workflow runs against the real GitHub secret before building.

If that job is green on `main`, both the preflight and the post-build verify step on a tag push will also be green. Check it at [github.com/chrisl8/trip-viewer/actions/workflows/verify-dry-run.yml](https://github.com/chrisl8/trip-viewer/actions/workflows/verify-dry-run.yml) before tagging.

You can also run the post-build verify locally after `npm run tauri build`:

```bash
bash scripts/verify-updater.sh
```

This auto-detects the bundle for your platform and verifies it against the pubkey in `src-tauri/tauri.conf.json`. It will download minisign on the fly if you don't have it installed.

## How to cut a release

From a clean working tree on `main`:

```bash
npm run release patch      # 0.1.4 → 0.1.5
npm run release minor      # 0.1.4 → 0.2.0
npm run release major      # 0.1.4 → 1.0.0
npm run release 0.2.0-rc1  # explicit version
```

That single command:

1. Verifies the working tree is clean and you're on `main`
2. Updates `package.json` and `package-lock.json` with the new version
3. Updates `src-tauri/Cargo.toml` (Tauri reads its version from `package.json` automatically via `"version": "../package.json"` in `tauri.conf.json`)
4. Runs `cargo check` to refresh `Cargo.lock`
5. Commits all four files with message `Bump version to X.Y.Z`
6. Creates the annotated tag `vX.Y.Z`
7. Pushes the commit and tag to `origin`

If the working tree is dirty or you're on the wrong branch, the script bails before making any changes.

### Wait for the build

The Action takes ~7-8 minutes. Monitor it at [github.com/chrisl8/trip-viewer/actions](https://github.com/chrisl8/trip-viewer/actions).

### Review and publish

The Action creates a **draft release**. Go to [github.com/chrisl8/trip-viewer/releases](https://github.com/chrisl8/trip-viewer/releases), edit the release notes if desired, and click **Publish release**.

The draft will have these assets attached:

**Windows:**

- `tripviewer_X.Y.Z_x64-setup.exe` — the NSIS installer
- `tripviewer_X.Y.Z_x64-setup.exe.sig` — updater signature

**Linux:**

- `trip-viewer_X.Y.Z_amd64.AppImage` — the AppImage (single-file binary, GStreamer plugins bundled)
- `trip-viewer_X.Y.Z_amd64.AppImage.sig` — updater signature

**Shared:**

- `latest.json` — auto-update manifest (contains signed URLs for both platforms)

## How the auto-updater works

When the app starts, it fetches `latest.json` from:

```
https://github.com/chrisl8/trip-viewer/releases/latest/download/latest.json
```

If a newer version is available, a toast notification appears in the bottom-right corner with an "Update & Restart" button. The update is verified against the public key before installing.

The updater only checks published (non-draft) releases.

On Linux (AppImage), the updater downloads the new AppImage, replaces the current binary on disk, and restarts. This works when the user launched the AppImage from a writable location; if it was installed read-only (e.g. inside `/opt` or via an integration tool), the updater will surface the error and the user can download manually from the Releases page. When a Flatpak build is eventually added, auto-update will be disabled inside the Flatpak — Flathub handles updates for sandboxed installs.

## Code signing (future)

The installer is currently unsigned, so Windows SmartScreen shows a warning on first install. Once there are a few public releases:

1. Apply at [signpath.org](https://signpath.org) for free open-source code signing
2. Once approved, add their GitHub Action step to `.github/workflows/release.yml`
3. Configure `certificateThumbprint` in `src-tauri/tauri.conf.json` under `bundle.windows`

Alternative: [Certum open-source code signing](https://certum.store/open-source-code-signing-code.html) (~25 EUR/year).

## Troubleshooting

### "Wrong password for that key"

The `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` GitHub secret doesn't match the password used when generating the key. GitHub secrets cannot be empty strings — if you need a passwordless key, regenerate with a real password.

### Action fails during build

Check the Actions log. Common causes:

- Rust compilation errors (test locally with `npm run tauri build` first)
- Missing GitHub secrets
- Tag doesn't match a commit that exists on the GitHub mirror

### SmartScreen warning on install

Expected until code signing is set up. Users click "More info" > "Run anyway". This goes away once the app builds SmartScreen reputation (or after code signing is added).

### Installer is very large

The NSIS installer should be ~3-6 MB. If it's much larger, check that `bundle.targets` in `tauri.conf.json` is set to `["nsis", "appimage"]` and not `"all"` (which would include MSI with embedded WebView2 at ~130 MB).

### Linux build fails on the GitHub runner

The Ubuntu job in `.github/workflows/release.yml` installs `webkit2gtk-4.1`, `gstreamer1.0-libav`, `gstreamer1.0-plugins-bad`, and the rest of Tauri's Linux build deps before running `tauri-action`. If the build fails during system-dep install, check whether the base Ubuntu image version still ships WebKit2GTK 4.1 — Tauri v2 needs 4.1, not the older 4.0. As of this release we target `ubuntu-22.04`.

### AppImage won't start on user's machine

If a user reports the AppImage exits immediately or complains about missing libraries, the most common causes are:

- No `libfuse2` on their system (required to mount the AppImage). On modern Ubuntu: `sudo apt install libfuse2`.
- Missing `gstreamer1.0-libav` — this one the app itself catches via the HEVC support gate and shows an install hint.
- Very old distro without WebKit2GTK 4.1 available — out of scope; we don't ship older Ubuntu LTS targets.
