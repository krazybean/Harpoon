#!/bin/bash
set -euo pipefail
# ponytail: fetch Alpine 3.22 virt kernel and produce uncompressed Image-virt
# Fixes historical prototype relative CACHE bug by using REPO_ROOT absolute.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT_DIR="$REPO_ROOT/assets/guest"
CACHE_DIR="$OUT_DIR/.cache"
mkdir -p "$OUT_DIR" "$CACHE_DIR"

BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot"
KERNEL_CACHE="$CACHE_DIR/vmlinuz-virt"
INITRAMFS_CACHE="$CACHE_DIR/initramfs-virt"
OUT_IMAGE="$OUT_DIR/Image-virt"

if [ ! -f "$KERNEL_CACHE" ]; then
  echo "[fetch-kernel] fetching vmlinuz-virt..." >&2
  curl -L --fail -o "$KERNEL_CACHE" "$BASE/vmlinuz-virt"
fi
if [ ! -f "$INITRAMFS_CACHE" ]; then
  echo "[fetch-kernel] fetching initramfs-virt..." >&2
  curl -L --fail -o "$INITRAMFS_CACHE" "$BASE/initramfs-virt"
fi

echo "[fetch-kernel] kernel $(stat -f%z "$KERNEL_CACHE" 2>/dev/null || stat -c%s "$KERNEL_CACHE") bytes" >&2
echo "[fetch-kernel] initramfs-virt $(stat -f%z "$INITRAMFS_CACHE" 2>/dev/null || stat -c%s "$INITRAMFS_CACHE") bytes" >&2
if command -v shasum >/dev/null; then
  echo "[fetch-kernel] sha256 vmlinuz: $(shasum -a 256 "$KERNEL_CACHE" | cut -d' ' -f1)" >&2
fi
echo "[fetch-kernel] source: $BASE (Alpine 3.22 virt 6.12.x)" >&2

# Decompress PE+gz wrapper to uncompressed Image
if [ ! -f "$OUT_IMAGE" ] || [ "$KERNEL_CACHE" -nt "$OUT_IMAGE" ]; then
  echo "[fetch-kernel] decompressing vmlinuz-virt -> Image-virt..." >&2
  python3 -c "
import pathlib, subprocess
data=pathlib.Path('$KERNEL_CACHE').read_bytes()
pos=data.find(b'\x1f\x8b\x08')
if pos < 0:
    raise SystemExit('gzip magic not found in vmlinuz-virt')
open('/tmp/_harpoon_chunk.gz','wb').write(data[pos:])
r=subprocess.run(['gunzip','-c','/tmp/_harpoon_chunk.gz'], capture_output=True)
if r.returncode != 0:
    raise SystemExit(r.stderr.decode())
open('$OUT_IMAGE','wb').write(r.stdout)
print(f'decompressed {len(r.stdout)} bytes')
"
  echo "[fetch-kernel] Image-virt $(stat -f%z "$OUT_IMAGE" 2>/dev/null || stat -c%s "$OUT_IMAGE") bytes $(file "$OUT_IMAGE" | head -c 120)" >&2
  if command -v shasum >/dev/null; then
    echo "[fetch-kernel] sha256 Image-virt: $(shasum -a 256 "$OUT_IMAGE" | cut -d' ' -f1)" >&2
  fi
else
  echo "[fetch-kernel] Image-virt up to date at $OUT_IMAGE" >&2
fi
echo "[fetch-kernel] done -> $OUT_IMAGE" >&2
