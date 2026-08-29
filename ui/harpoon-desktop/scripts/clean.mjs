#!/usr/bin/env node
import fs from 'fs';
import path from 'path';
import { spawnSync } from 'child_process';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const uiRoot = path.resolve(__dirname, '..');

const targets = [
  'src-tauri/target',
  'src-tauri/bundle-resources',
  'dist'
];

const args = process.argv.slice(2);
const dryRun = args.includes('--dry-run');

function du(paths) {
  const res = spawnSync('du', ['-sh', ...paths.map(p => path.join(uiRoot, p))], { encoding: 'utf8' });
  if (res.status === 0) console.log(res.stdout.trim());
}

console.log(dryRun ? '[clean] --dry-run preview:' : '[clean] removing:');
for (const p of targets) {
  const full = path.join(uiRoot, p);
  if (fs.existsSync(full)) {
    console.log(`  ${p}`);
  } else {
    console.log(`  ${p} (not present)`);
  }
}

if (dryRun) {
  console.log('\n[dry-run] no files removed');
  du(targets);
  process.exit(0);
}

let before = 0;
try {
  const res = spawnSync('du', ['-sb', ...targets.map(p => path.join(uiRoot, p))], { encoding: 'utf8' });
  if (res.status === 0) {
    for (const line of res.stdout.trim().split('\n')) {
      const [size] = line.split('\t');
      before += parseInt(size, 10) || 0;
    }
  }
} catch {}

for (const p of targets) {
  const full = path.join(uiRoot, p);
  if (fs.existsSync(full)) {
    fs.rmSync(full, { recursive: true, force: true });
  }
}

let after = 0;
try {
  const res2 = spawnSync('du', ['-sb', ...targets.map(p => path.join(uiRoot, p))], { encoding: 'utf8' });
  if (res2.status === 0) {
    for (const line of res2.stdout.trim().split('\n')) {
      after += parseInt(size, 10) || 0;
    }
  }
} catch {}

console.log(`[clean] done. reclaimed ~${(before/1024/1024).toFixed(1)} MiB (before ${before} bytes)`);
console.log('DO NOT removed: assets/guest/Image-virt, assets/guest/harpoon-initramfs.cpio.gz, assets/guest/harpoon-root.img, ~/Library/Application Support/Harpoon, /tmp/harpoon-runtime, harpoon/results, docs/results (spike1/spike2 historical)');
