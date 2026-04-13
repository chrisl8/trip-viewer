# Releasing Trip Viewer

Pushing a version tag (e.g. `v0.2.0`) to the GitHub mirror triggers a GitHub Actions workflow that builds a Windows NSIS installer, signs it for the auto-updater, and creates a draft release with the installer and update manifest attached. You review the draft, edit the release notes if desired, and publish it.

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

## How to cut a release

### 1. Bump the version

Update the version string in all three places:

| File                        | Field                |
| --------------------------- | -------------------- |
| `src-tauri/Cargo.toml`      | `version = "X.Y.Z"`  |
| `src-tauri/tauri.conf.json` | `"version": "X.Y.Z"` |
| `package.json`              | `"version": "X.Y.Z"` |

### 2. Commit the version bump

```bash
git add src-tauri/tauri.conf.json package.json src-tauri/Cargo.toml
git commit -m "Bump version to X.Y.Z"
```

### 3. Tag the release

```bash
git tag vX.Y.Z
```

### 4. Push to GitHub

If your GitHub remote is named `github`:

```bash
git push github main
git push github vX.Y.Z
```

If you use Forgejo push mirroring, the tag may sync automatically — verify at github.com/chrisl8/trip-viewer/actions.

### 5. Wait for the build

The Action takes ~7-8 minutes. Monitor it at [github.com/chrisl8/trip-viewer/actions](https://github.com/chrisl8/trip-viewer/actions).

### 6. Review and publish

The Action creates a **draft release**. Go to [github.com/chrisl8/trip-viewer/releases](https://github.com/chrisl8/trip-viewer/releases), edit the release notes if desired, and click **Publish release**.

The draft will have these assets attached:

- `tripviewer_X.Y.Z_x64-setup.exe` — the NSIS installer
- `tripviewer_X.Y.Z_x64-setup.exe.sig` — updater signature
- `latest.json` — auto-update manifest

## How the auto-updater works

When the app starts, it fetches `latest.json` from:

```
https://github.com/chrisl8/trip-viewer/releases/latest/download/latest.json
```

If a newer version is available, a toast notification appears in the bottom-right corner with an "Update & Restart" button. The update is verified against the public key before installing.

The updater only checks published (non-draft) releases.

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

The NSIS installer should be ~3-6 MB. If it's much larger, check that `bundle.targets` in `tauri.conf.json` is set to `["nsis"]` and not `"all"` (which would include MSI with embedded WebView2 at ~130 MB).
