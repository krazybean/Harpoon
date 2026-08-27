#!/usr/bin/env node
// Ponytail: single source of truth package.json, sync only if required, no semver dep
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');
const pkgPath = path.join(root, 'package.json');
const tauriPath = path.join(root, 'src-tauri/tauri.conf.json');
const cargoTomlPath = path.join(root, 'src-tauri/Cargo.toml');
const cargoLockPath = path.join(root, 'src-tauri/Cargo.lock');
const pkgLockPath = path.join(root, 'package-lock.json');

function readJson(p) { return JSON.parse(fs.readFileSync(p, 'utf8')); }
function writeJsonAtomic(p, obj) {
  const tmp = p + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(obj, null, 2) + '\n');
  fs.renameSync(tmp, p);
}
function isSemver(v) {
  return /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/.test(v);
}
function parse(v) {
  const m = v.match(/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/);
  if (!m) throw new Error(`invalid semver ${v}`);
  return { major: +m[1], minor: +m[2], patch: +m[3], prerelease: m[4] || null };
}
function format(p) {
  let s = `${p.major}.${p.minor}.${p.patch}`;
  if (p.prerelease) s += `-${p.prerelease}`;
  return s;
}
function bump(current, kind) {
  if (isSemver(kind)) return kind; // explicit
  const cur = parse(current);
  if (kind === 'patch') {
    cur.patch += 1;
    cur.prerelease = null;
    return format(cur);
  }
  if (kind === 'minor') {
    cur.minor += 1;
    cur.patch = 0;
    cur.prerelease = null;
    return format(cur);
  }
  if (kind === 'major') {
    cur.major += 1;
    cur.minor = 0;
    cur.patch = 0;
    cur.prerelease = null;
    return format(cur);
  }
  if (kind === 'prerelease') {
    if (cur.prerelease) {
      // bump last numeric part
      const parts = cur.prerelease.split('.');
      const last = parts[parts.length - 1];
      const num = parseInt(last, 10);
      if (!isNaN(num)) {
        parts[parts.length - 1] = String(num + 1);
      } else {
        parts.push('0');
      }
      cur.prerelease = parts.join('.');
    } else {
      cur.patch += 1;
      cur.prerelease = '0';
    }
    return format(cur);
  }
  throw new Error(`unknown bump kind ${kind} (expected patch|minor|major|prerelease or explicit semver)`);
}

function updateCargoToml(p, newVer) {
  if (!fs.existsSync(p)) return;
  let txt = fs.readFileSync(p, 'utf8');
  const re = /^version\s*=\s*"[^"]*"/m;
  // Only update [package] version, first occurrence
  if (!re.test(txt)) return;
  const lines = txt.split('\n');
  let inPackage = false;
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].trim() === '[package]') inPackage = true;
    else if (lines[i].startsWith('[')) inPackage = false;
    if (inPackage && /^version\s*=/.test(lines[i])) {
      lines[i] = `version = "${newVer}"`;
      break;
    }
  }
  const out = lines.join('\n');
  if (out !== txt) {
    fs.writeFileSync(p + '.tmp', out);
    fs.renameSync(p + '.tmp', p);
  }
}

function updateCargoLock(p, newVer) {
  if (!fs.existsSync(p)) return;
  let txt = fs.readFileSync(p, 'utf8');
  // Replace harpoon-desktop version
  const re = /(\[\[package\]\]\nname = "harpoon-desktop"\nversion = ")[^"]*(")/;
  if (re.test(txt)) {
    const out = txt.replace(re, `$1${newVer}$2`);
    if (out !== txt) {
      fs.writeFileSync(p + '.tmp', out);
      fs.renameSync(p + '.tmp', p);
    }
  }
}

function updatePackageLock(p, newVer) {
  if (!fs.existsSync(p)) return;
  const j = readJson(p);
  let changed = false;
  if (j.version !== newVer) { j.version = newVer; changed = true; }
  if (j.packages && j.packages[""] && j.packages[""].version !== newVer) {
    j.packages[""].version = newVer; changed = true;
  }
  // Also update reference to harpoon-desktop package if present
  // package-lock for ui/harpoon-desktop is root, so above covers
  if (changed) writeJsonAtomic(p, j);
}

const args = process.argv.slice(2);
const cmd = args[0];

// version:check
if (cmd === '--check' || cmd === 'check' || args.includes('--check')) {
  const pkg = readJson(pkgPath);
  const pkgVer = pkg.version;
  console.log(`package.json       ${pkgVer}`);
  let ok = true;
  // tauri.conf.json
  if (fs.existsSync(tauriPath)) {
    const tauri = readJson(tauriPath);
    const tVer = tauri.version;
    // If version is a path (contains / or \), it's a reference, not a semver, skip check
    if (typeof tVer === 'string' && tVer.includes('/')) {
      console.log(`tauri.conf.json    ${tVer} (reference, skip)`);
    } else if (tVer) {
      console.log(`tauri.conf.json    ${tVer}`);
      if (tVer !== pkgVer) { ok = false; console.error(`mismatch tauri.conf.json ${tVer} != ${pkgVer}`); }
    } else {
      console.log(`tauri.conf.json    (no version, uses Cargo.toml)`);
    }
  }
  if (fs.existsSync(cargoTomlPath)) {
    const txt = fs.readFileSync(cargoTomlPath, 'utf8');
    const m = txt.match(/^version\s*=\s*"([^"]+)"/m);
    const cVer = m ? m[1] : null;
    // Find first [package] version
    const lines = txt.split('\n');
    let inPkg = false, ver = null;
    for (const l of lines) {
      if (l.trim() === '[package]') inPkg = true;
      else if (l.startsWith('[')) inPkg = false;
      if (inPkg) {
        const mm = l.match(/^version\s*=\s*"([^"]+)"/);
        if (mm) { ver = mm[1]; break; }
      }
    }
    console.log(`Cargo.toml         ${ver}`);
    if (ver && ver !== pkgVer) { ok = false; console.error(`mismatch Cargo.toml ${ver} != ${pkgVer}`); }
  }
  if (fs.existsSync(cargoLockPath)) {
    const txt = fs.readFileSync(cargoLockPath, 'utf8');
    const m = txt.match(/\[\[package\]\]\nname = "harpoon-desktop"\nversion = "([^"]+)"/);
    const lVer = m ? m[1] : null;
    console.log(`Cargo.lock         ${lVer}`);
    if (lVer && lVer !== pkgVer) { ok = false; console.error(`mismatch Cargo.lock ${lVer} != ${pkgVer}`); }
  }
  if (fs.existsSync(pkgLockPath)) {
    const j = readJson(pkgLockPath);
    const plVer = j.version;
    console.log(`package-lock.json  ${plVer}`);
    if (plVer !== pkgVer) { ok = false; console.error(`mismatch package-lock.json ${plVer} != ${pkgVer}`); }
  }
  console.log(`\nVersion consistency: ${ok ? 'PASS' : 'FAIL'}`);
  process.exit(ok ? 0 : 1);
}

// version:bump
const bumpArg = args[0];
if (!bumpArg) {
  console.error('Usage: npm run version:bump -- <patch|minor|major|prerelease|1.2.3>');
  process.exit(1);
}
if (!fs.existsSync(pkgPath)) { console.error('package.json not found'); process.exit(1); }
const pkg = readJson(pkgPath);
const current = pkg.version;
if (!isSemver(current)) { console.error(`current version invalid: ${current}`); process.exit(1); }
let target;
try {
  target = bump(current, bumpArg);
} catch (e) {
  console.error(e.message);
  process.exit(1);
}
if (!isSemver(target)) { console.error(`target version invalid: ${target}`); process.exit(1); }

// fail before modifying if target same?
console.log(`Harpoon ${current} -> ${target}`);

// Update files atomically
pkg.version = target;
writeJsonAtomic(pkgPath, pkg);

// tauri.conf.json - only if it contains explicit semver, not a reference path
if (fs.existsSync(tauriPath)) {
  const tauri = readJson(tauriPath);
  const tVer = tauri.version;
  if (typeof tVer === 'string' && !tVer.includes('/') && isSemver(tVer)) {
    tauri.version = target;
    writeJsonAtomic(tauriPath, tauri);
  }
}

updateCargoToml(cargoTomlPath, target);
updateCargoLock(cargoLockPath, target);
updatePackageLock(pkgLockPath, target);

console.log(`updated package.json, tauri.conf.json (if explicit), Cargo.toml, Cargo.lock, package-lock.json`);
