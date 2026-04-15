#!/usr/bin/env node
// Release script: bumps version across package.json and Cargo.toml,
// updates Cargo.lock, commits, tags, and pushes to origin.
//
// Usage:
//   npm run release patch      # 0.1.4 → 0.1.5
//   npm run release minor      # 0.1.4 → 0.2.0
//   npm run release major      # 0.1.4 → 1.0.0
//   npm run release 0.2.0-rc1  # explicit version

import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const pkgPath = join(repo, "package.json");
const lockPath = join(repo, "package-lock.json");
const cargoPath = join(repo, "src-tauri", "Cargo.toml");

// ── Utilities ───────────────────────────────────────────────────────────────

function run(cmd, opts = {}) {
  return execSync(cmd, { cwd: repo, stdio: "pipe", encoding: "utf8", ...opts }).trim();
}

function runInherit(cmd) {
  return execSync(cmd, { cwd: repo, stdio: "inherit" });
}

function bail(msg) {
  console.error(`\n\x1b[31mERROR:\x1b[0m ${msg}\n`);
  process.exit(1);
}

function info(msg) {
  console.log(`\x1b[36m→\x1b[0m ${msg}`);
}

function ok(msg) {
  console.log(`\x1b[32m✓\x1b[0m ${msg}`);
}

function bumpSemver(current, kind) {
  const [major, minor, patch] = current.split(/[-+]/)[0].split(".").map(Number);
  switch (kind) {
    case "major":
      return `${major + 1}.0.0`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    default:
      bail(`unknown bump kind: ${kind}`);
  }
}

function isValidSemver(v) {
  return /^\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$/.test(v);
}

// ── Prechecks ───────────────────────────────────────────────────────────────

const arg = process.argv[2];
if (!arg) {
  bail("usage: npm run release <patch|minor|major|X.Y.Z>");
}

info("Checking working tree is clean...");
const status = run("git status --porcelain");
if (status) {
  bail(
    "Working tree is not clean. Commit or stash changes before releasing.\n" +
      status,
  );
}

info("Checking current branch...");
const branch = run("git rev-parse --abbrev-ref HEAD");
if (branch !== "main") {
  bail(`Must be on 'main' branch to release. Currently on '${branch}'.`);
}

info("Checking remote 'origin' exists...");
try {
  run("git remote get-url origin");
} catch {
  bail("No git remote named 'origin' is configured.");
}

// ── Compute new version ─────────────────────────────────────────────────────

const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const currentVersion = pkg.version;
info(`Current version: ${currentVersion}`);

let newVersion;
if (arg === "patch" || arg === "minor" || arg === "major") {
  newVersion = bumpSemver(currentVersion, arg);
} else {
  if (!isValidSemver(arg)) {
    bail(`'${arg}' is not a valid semver version (e.g. 1.2.3 or 1.2.3-rc1).`);
  }
  newVersion = arg;
}

info(`New version:     ${newVersion}`);

// ── Update files ────────────────────────────────────────────────────────────

info("Updating package.json...");
pkg.version = newVersion;
writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");

info("Updating package-lock.json...");
const lock = JSON.parse(readFileSync(lockPath, "utf8"));
lock.version = newVersion;
if (lock.packages && lock.packages[""]) {
  lock.packages[""].version = newVersion;
}
writeFileSync(lockPath, JSON.stringify(lock, null, 2) + "\n");

info("Updating src-tauri/Cargo.toml...");
const cargo = readFileSync(cargoPath, "utf8");
// Match `version = "..."` in the [package] section. Cargo.toml has a
// strict format, so a simple line-based replace is safe here.
const cargoNew = cargo.replace(
  /^version\s*=\s*"[^"]+"/m,
  `version = "${newVersion}"`,
);
if (cargoNew === cargo) {
  bail("Failed to update version in Cargo.toml — regex didn't match.");
}
writeFileSync(cargoPath, cargoNew);

info("Updating Cargo.lock via cargo check...");
try {
  execSync("cargo check --manifest-path src-tauri/Cargo.toml --quiet", {
    cwd: repo,
    stdio: "inherit",
  });
} catch {
  bail("cargo check failed — fix compilation errors before releasing.");
}

// ── Commit ──────────────────────────────────────────────────────────────────

info("Staging version files...");
run("git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock");

info(`Committing: Bump version to ${newVersion}`);
run(`git commit -m "Bump version to ${newVersion}"`);

// ── Tag ─────────────────────────────────────────────────────────────────────

const tag = `v${newVersion}`;
info(`Tagging: ${tag}`);
run(`git tag -a ${tag} -m "Release ${newVersion}"`);

// ── Push ────────────────────────────────────────────────────────────────────

info("Pushing commit and tag to origin...");
runInherit("git push origin main");
runInherit(`git push origin ${tag}`);

ok(`Released ${tag}`);
console.log(
  `\nGitHub Actions will build and create a draft release.\nReview and publish at: https://github.com/chrisl8/trip-viewer/releases`,
);
