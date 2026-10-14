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
EXPECTED_VMLINUZ_SHA="dd37c4460f933ec909ff022c34944ff7e1a5300aff99750e3bdc02cd4074c332"
EXPECTED_INITRAMFS_VIRT_SHA="384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c"

sha256_file() {
  shasum -a 256 "$1" | cut -d' ' -f1
}

fetch_pinned() {
  local name="$1" expected="$2" cache="$3" tmp actual
  if [ -f "$cache" ]; then
    actual=$(sha256_file "$cache")
    if [ "$actual" = "$expected" ]; then
      echo "[fetch-kernel] reusing pinned $name ($actual)" >&2
      return 0
    fi
    echo "[fetch-kernel] stale $name cache: expected $expected got $actual; refreshing" >&2
  else
    echo "[fetch-kernel] fetching $name..." >&2
  fi
  tmp="$cache.tmp"
  curl -L --fail -o "$tmp" "$BASE/$name"
  actual=$(sha256_file "$tmp")
  if [ "$actual" != "$expected" ]; then
    echo "[fetch-kernel] FAIL: $name SHA mismatch expected $expected got $actual" >&2
    rm -f "$tmp"
    return 1
  fi
  mv "$tmp" "$cache"
}

fetch_pinned vmlinuz-virt "$EXPECTED_VMLINUZ_SHA" "$KERNEL_CACHE"
fetch_pinned initramfs-virt "$EXPECTED_INITRAMFS_VIRT_SHA" "$INITRAMFS_CACHE"

echo "[fetch-kernel] kernel $(stat -f%z "$KERNEL_CACHE" 2>/dev/null || stat -c%s "$KERNEL_CACHE") bytes" >&2
echo "[fetch-kernel] initramfs-virt $(stat -f%z "$INITRAMFS_CACHE" 2>/dev/null || stat -c%s "$INITRAMFS_CACHE") bytes" >&2
if command -v shasum >/dev/null; then
  echo "[fetch-kernel] sha256 vmlinuz: $(shasum -a 256 "$KERNEL_CACHE" | cut -d' ' -f1)" >&2
fi
echo "[fetch-kernel] source: $BASE (Alpine 3.22 virt 6.12.x)" >&2

# Decompress PE+gz wrapper to uncompressed Image
if [ ! -f "$OUT_IMAGE" ] || [ "$KERNEL_CACHE" -nt "$OUT_IMAGE" ]; then
  echo "[fetch-kernel] decompressing vmlinuz-virt -> Image-virt..." >&2
  python3 - "$KERNEL_CACHE" "$OUT_IMAGE" <<'PY'
import pathlib
import sys
import zlib

source, destination = map(pathlib.Path, sys.argv[1:])
data = source.read_bytes()
pos = data.find(b"\x1f\x8b\x08")
if pos < 0:
    raise SystemExit("gzip magic not found in vmlinuz-virt")

decoder = zlib.decompressobj(16 + zlib.MAX_WBITS)
try:
    image = decoder.decompress(data[pos:]) + decoder.flush()
except zlib.error as exc:
    raise SystemExit(f"gzip decompression failed: {exc}")
if not decoder.eof:
    raise SystemExit("gzip member ended before its end marker")

# vmlinuz-virt is a PE wrapper; unused_data is unrelated bytes after the gzip member.
destination.write_bytes(image)
print(f"decompressed {len(image)} bytes")
PY
  echo "[fetch-kernel] Image-virt $(stat -f%z "$OUT_IMAGE" 2>/dev/null || stat -c%s "$OUT_IMAGE") bytes $(file "$OUT_IMAGE" | head -c 120)" >&2
  if command -v shasum >/dev/null; then
    echo "[fetch-kernel] sha256 Image-virt: $(shasum -a 256 "$OUT_IMAGE" | cut -d' ' -f1)" >&2
  fi
else
  echo "[fetch-kernel] Image-virt up to date at $OUT_IMAGE" >&2
fi
echo "[fetch-kernel] done -> $OUT_IMAGE" >&2
