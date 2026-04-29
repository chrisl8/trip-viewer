#!/usr/bin/env node
// Periodic cargo-sweep for src-tauri/target.
//
// Wired into package.json as `pretauri`, so it runs before any
// `npm run tauri ...` invocation. Gated by a stamp file so the actual sweep
// only happens about once a week — most invocations are a no-op fast-path.
//
// Removes build artifacts not touched in the last 14 days. cargo-sweep falls
// back to mtime on Windows where atime updates may be disabled.
//
// Requires `cargo install cargo-sweep`. Missing binary is a soft-fail (warn
// and continue) so a fresh clone never blocks on it.

import { existsSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SWEEP_INTERVAL_DAYS = 7;
const ARTIFACT_TTL_DAYS = 14;

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const tauriDir = join(repoRoot, "src-tauri");
const targetDir = join(tauriDir, "target");
const stampFile = join(targetDir, ".tripviewer-last-sweep");

if (!existsSync(targetDir)) {
  process.exit(0);
}

if (existsSync(stampFile)) {
  const ageDays = (Date.now() - statSync(stampFile).mtimeMs) / 86_400_000;
  if (ageDays < SWEEP_INTERVAL_DAYS) {
    process.exit(0);
  }
}

// Direct spawn (no `shell: true`) — Node resolves cargo.exe via PATH and
// the args array preserves paths with spaces verbatim.
const cargoBin = process.platform === "win32" ? "cargo.exe" : "cargo";

const versionCheck = spawnSync(cargoBin, ["sweep", "--version"], { stdio: "ignore" });
if (versionCheck.status !== 0) {
  console.warn(
    "[cargo-sweep] not installed — skipping periodic prune. Install with: cargo install cargo-sweep",
  );
  process.exit(0);
}

console.log(
  `[cargo-sweep] pruning src-tauri/target artifacts unused for >${ARTIFACT_TTL_DAYS} days...`,
);
const sweep = spawnSync(
  cargoBin,
  ["sweep", "--time", String(ARTIFACT_TTL_DAYS), tauriDir],
  { stdio: "inherit" },
);

if (sweep.status === 0) {
  writeFileSync(stampFile, new Date().toISOString());
} else {
  console.warn("[cargo-sweep] sweep failed — will retry on next invocation");
}
