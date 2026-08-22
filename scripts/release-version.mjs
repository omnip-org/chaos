#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const argumentsList = process.argv.slice(2);
const checkOnly = argumentsList[0] === "--check";
const rawVersion = argumentsList[checkOnly ? 1 : 0];

if (!rawVersion || argumentsList.length !== (checkOnly ? 2 : 1)) {
  console.error("Usage: release-version.mjs [--check] <version-or-tag>");
  process.exit(2);
}

const version = rawVersion.startsWith("v") ? rawVersion.slice(1) : rawVersion;
if (!isSemver(version)) {
  console.error(`Invalid release version: ${rawVersion}`);
  process.exit(2);
}

const cargoManifestPath = path.join(repositoryRoot, "Cargo.toml");
const cargoLockPath = path.join(repositoryRoot, "Cargo.lock");
const sdkPackagePath = path.join(repositoryRoot, "packages/js/package.json");
const sdkLockPath = path.join(repositoryRoot, "packages/js/package-lock.json");

const cargoManifest = read(cargoManifestPath);
const cargoLock = read(cargoLockPath);
const sdkPackage = read(sdkPackagePath);
const sdkLock = read(sdkLockPath);

const workspacePackageNames = getWorkspacePackageNames(cargoManifest);
const updates = [
  {
    path: cargoManifestPath,
    current: cargoManifest,
    next: replaceWorkspaceVersion(cargoManifest, version),
  },
  {
    path: cargoLockPath,
    current: cargoLock,
    next: replaceCargoLockVersions(cargoLock, workspacePackageNames, version),
  },
  {
    path: sdkPackagePath,
    current: sdkPackage,
    next: replaceJsonVersion(sdkPackage, version, 1),
  },
  {
    path: sdkLockPath,
    current: sdkLock,
    next: replaceJsonVersion(sdkLock, version, 2),
  },
];

const changed = updates.filter((file) => file.current !== file.next);
if (checkOnly) {
  if (changed.length > 0) {
    console.error(
      `Release version ${version} is not synchronized in: ${changed
        .map((file) => path.relative(repositoryRoot, file.path))
        .join(", ")}`,
    );
    process.exit(1);
  }
  console.log(`Release version ${version} is synchronized.`);
} else {
  for (const file of changed) fs.writeFileSync(file.path, file.next);
  console.log(
    changed.length === 0
      ? `Release version ${version} is already synchronized.`
      : `Synchronized release version ${version} in ${changed.length} files.`,
  );
}

function read(filePath) {
  return fs.readFileSync(filePath, "utf8");
}

function isSemver(value) {
  return /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(
    value,
  );
}

function getWorkspacePackageNames(manifest) {
  const membersMatch = manifest.match(/\[workspace\][\s\S]*?members\s*=\s*\[([\s\S]*?)\]/);
  if (!membersMatch) throw new Error("Cargo.toml is missing [workspace].members");

  return [...membersMatch[1].matchAll(/"([^"]+)"/g)].map((match) => {
    const memberManifest = read(path.join(repositoryRoot, match[1], "Cargo.toml"));
    const packageName = memberManifest.match(/^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m);
    if (!packageName) throw new Error(`Cannot find package name in ${match[1]}/Cargo.toml`);
    return packageName[1];
  });
}

function replaceWorkspaceVersion(manifest, nextVersion) {
  const pattern = /(\[workspace\.package\][\s\S]*?^version\s*=\s*")([^"]+)("\s*$)/m;
  const match = manifest.match(pattern);
  if (!match) throw new Error("Cargo.toml workspace version was not found");
  if (match[2] === nextVersion) return manifest;
  return manifest.replace(pattern, `$1${nextVersion}$3`);
}

function replaceCargoLockVersions(lockfile, packageNames, nextVersion) {
  let updated = lockfile;
  for (const packageName of packageNames) {
    const escapedName = packageName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const pattern = new RegExp(
      `(\\[\\[package\\]\\]\\nname = "${escapedName}"\\nversion = ")([^"]+)("$)`,
      "m",
    );
    const match = updated.match(pattern);
    if (!match) throw new Error(`Cargo.lock entry not found for ${packageName}`);
    if (match[2] !== nextVersion) updated = updated.replace(pattern, `$1${nextVersion}$3`);
  }
  return updated;
}

function replaceJsonVersion(jsonText, nextVersion, occurrences) {
  const parsed = JSON.parse(jsonText);
  if (typeof parsed.version !== "string") throw new Error("JSON package version is missing");
  if (occurrences === 2 && parsed.packages?.[""]?.version === undefined) {
    throw new Error("Root package version is missing from package-lock.json");
  }

  let count = 0;
  const updated = jsonText.replace(/("version"\s*:\s*")[^"]+("\s*)/g, (match, prefix, suffix) => {
    if (count >= occurrences) return match;
    count += 1;
    return `${prefix}${nextVersion}${suffix}`;
  });
  if (count !== occurrences) throw new Error("Expected JSON package version fields were not found");
  return updated;
}
