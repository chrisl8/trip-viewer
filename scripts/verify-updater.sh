#!/usr/bin/env bash
# Verify a Tauri updater artifact against its .sig using the pubkey in tauri.conf.json.
# Works on Windows (Git Bash), Linux, macOS. Installs minisign if not on PATH.
#
# Usage:  scripts/verify-updater.sh [<artifact-path>]
#
# If <artifact-path> is omitted, globs the default bundle dir for the current platform.
# Override the config file with the TAURI_CONF env var (used by the dry-run workflow).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONF="${TAURI_CONF:-$REPO_ROOT/src-tauri/tauri.conf.json}"

case "${OSTYPE:-$(uname -s)}" in
  msys*|cygwin*|mingw*|MINGW*|MSYS*) PLATFORM=windows ;;
  linux*|Linux*)                     PLATFORM=linux   ;;
  darwin*|Darwin*)                   PLATFORM=macos   ;;
  *) echo "verify-updater: unsupported platform: ${OSTYPE:-$(uname -s)}" >&2; exit 2 ;;
esac

EXPLICIT="${1:-}"
if [ -n "$EXPLICIT" ]; then
  ARTIFACT="$EXPLICIT"
  if [ ! -f "$ARTIFACT" ]; then
    echo "verify-updater: artifact not found: $ARTIFACT" >&2
    exit 2
  fi
else
  case "$PLATFORM" in
    windows) GLOB="$REPO_ROOT/src-tauri/target/release/bundle/nsis/*_x64-setup.exe" ;;
    linux)   GLOB="$REPO_ROOT/src-tauri/target/release/bundle/appimage/*_amd64.AppImage" ;;
    macos)   GLOB="$REPO_ROOT/src-tauri/target/release/bundle/macos/*.app.tar.gz" ;;
  esac
  # shellcheck disable=SC2086
  ARTIFACT=$(ls -1 $GLOB 2>/dev/null | head -n 1 || true)
  if [ -z "${ARTIFACT:-}" ] || [ ! -f "$ARTIFACT" ]; then
    echo "::warning::verify-updater: no artifact matched $GLOB — skipping"
    exit 0
  fi
fi

SIG="${ARTIFACT}.sig"
if [ ! -f "$SIG" ]; then
  echo "::warning::verify-updater: no .sig next to $ARTIFACT — updater not enabled for this target"
  exit 0
fi

if ! command -v minisign >/dev/null 2>&1; then
  TMP_INSTALL="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/minisign-install-$$"
  mkdir -p "$TMP_INSTALL"
  case "$PLATFORM" in
    windows)
      echo "verify-updater: installing minisign into $TMP_INSTALL"
      curl -fsSL -o "$TMP_INSTALL/minisign.zip" \
        https://github.com/jedisct1/minisign/releases/download/0.11/minisign-0.11-win64.zip
      unzip -q "$TMP_INSTALL/minisign.zip" -d "$TMP_INSTALL"
      export PATH="$TMP_INSTALL/minisign-win64:$PATH"
      ;;
    linux)
      echo "verify-updater: installing minisign into $TMP_INSTALL"
      curl -fsSL -o "$TMP_INSTALL/minisign.tar.gz" \
        https://github.com/jedisct1/minisign/releases/download/0.11/minisign-0.11-linux.tar.gz
      tar -xzf "$TMP_INSTALL/minisign.tar.gz" -C "$TMP_INSTALL"
      export PATH="$TMP_INSTALL/minisign-linux/x86_64:$PATH"
      ;;
    macos)
      if command -v brew >/dev/null 2>&1; then
        brew install minisign
      else
        echo "verify-updater: brew not found; please install minisign manually" >&2
        exit 2
      fi
      ;;
  esac
fi

if ! command -v minisign >/dev/null 2>&1; then
  echo "verify-updater: minisign install failed" >&2
  exit 2
fi

TMP_KEY="$(mktemp)"
trap 'rm -f "$TMP_KEY"' EXIT

# Node is already a hard dep (Tauri needs it) and is more portable than jq on dev
# machines. Path comes via argv so Git Bash does MSYS path translation correctly.
# On Git Bash its stdout can still become CRLF, hence `tr -d '\r\n\t '` before
# `base64 -d` — that stray \r is what broke the v0.1.18 Windows CI verify step.
node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).plugins.updater.pubkey)' "$CONF" \
  | tr -d '\r\n\t ' | base64 -d > "$TMP_KEY"

if [ ! -s "$TMP_KEY" ]; then
  echo "verify-updater: failed to decode pubkey from $CONF" >&2
  exit 2
fi

echo "verify-updater: verifying $(basename "$ARTIFACT") against its .sig"
minisign -Vm "$ARTIFACT" -x "$SIG" -p "$TMP_KEY"
echo "verify-updater: OK"
