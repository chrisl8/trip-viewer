#!/usr/bin/env bash
# Preflight: confirm the TAURI_SIGNING_PRIVATE_KEY env var produces signatures
# the pubkey in src-tauri/tauri.conf.json will accept.
#
# This catches the class of bug where the private key in GitHub Secrets and the
# embedded pubkey are a "keynum match, key-material mismatch" — the exact
# failure mode of v0.1.0–v0.1.14 (see commit 4e97198). Running this before the
# 7-minute tauri-action build means a mis-rotation fails fast with a clear
# error instead of producing unverifiable installers.
#
# Approach: sign a stub with the tauri CLI signer (same codepath tauri-action
# uses), then hand the stub + .sig to scripts/verify-updater.sh, which
# validates against the config pubkey. If that succeeds, the key pair is
# consistent with the config.
#
# Usage:  scripts/check-signing-key.sh
#
# Env (one of the two key sources is required; the tauri CLI reads both):
#   TAURI_SIGNING_PRIVATE_KEY           — private key as a string
#   TAURI_SIGNING_PRIVATE_KEY_PATH      — path to a private key file
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  — password for the key (required)
#   TAURI_CONF                          — path to tauri.conf.json
#                                         (optional; defaults to src-tauri/tauri.conf.json)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]; then
  echo "check-signing-key: set TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH" >&2
  exit 2
fi
: "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:?TAURI_SIGNING_PRIVATE_KEY_PASSWORD must be set}"

CONF="${TAURI_CONF:-$REPO_ROOT/src-tauri/tauri.conf.json}"
if [ ! -f "$CONF" ]; then
  echo "check-signing-key: config not found: $CONF" >&2
  exit 2
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

STUB="$TMP_DIR/preflight.bin"
printf 'check-signing-key preflight %s\n' "$(date -u +%s)" > "$STUB"

echo "check-signing-key: signing stub with TAURI_SIGNING_PRIVATE_KEY"
# tauri CLI reads the key and password from env vars; no need to put secrets
# on the argv.
(cd "$REPO_ROOT" && npx --no-install tauri signer sign "$STUB")

if [ ! -f "$STUB.sig" ]; then
  echo "check-signing-key: tauri signer did not produce $STUB.sig" >&2
  exit 2
fi

echo "check-signing-key: verifying stub.sig against pubkey in $CONF"
TAURI_CONF="$CONF" bash "$SCRIPT_DIR/verify-updater.sh" "$STUB"

echo "check-signing-key: OK — private key and config pubkey agree"
