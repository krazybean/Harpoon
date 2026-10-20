#!/bin/bash
set -euo pipefail
# ponytail: canonical production guest build — authoritative for assets/guest/
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/guest-arch.sh"
OUT_DIR="$(guest_asset_dir "$REPO_ROOT/assets/guest")"
mkdir -p "$OUT_DIR"
source "$SCRIPT_DIR/build-engine.sh"

echo "[guest-builder] building canonical guest assets -> $OUT_DIR" >&2
echo "[guest-builder] repo: $REPO_ROOT" >&2

# 1. Kernel (Image-virt) — fetch + decompress
echo "[guest-builder] step 1/3: kernel Image-virt" >&2
bash "$SCRIPT_DIR/fetch-kernel.sh"

# 2. Root image — clean Alpine population in a disposable VZ builder. Initramfs
# fingerprints and copies files from this image, so it must exist before that step.
echo "[guest-builder] step 2/3: root harpoon-root.img" >&2
bash "$SCRIPT_DIR/build-root.sh" --builder vz --force

# 3. Initramfs — fetch or rebuild (two-mode, no historical cache)
echo "[guest-builder] step 3/3: initramfs harpoon-initramfs.cpio.gz" >&2
bash "$SCRIPT_DIR/build-initramfs.sh"

# Final verification
echo "[guest-builder] verifying all canonical assets..." >&2
for f in "$OUT_DIR/Image-virt" "$OUT_DIR/harpoon-initramfs.cpio.gz" "$OUT_DIR/harpoon-root.img"; do
  if [ ! -f "$f" ]; then
    echo "[guest-builder] FAIL: missing $f" >&2
    exit 1
  fi
  ls -lh "$f" >&2
  du -h "$f" >&2 | tail -n 1 >&2
done

# Emit the pair contract only after an explicit complete guest build.
bash "$SCRIPT_DIR/guest-assets-manifest.sh" --write "$OUT_DIR"

# Canonical size check for root
SZ=$(stat -f%z "$OUT_DIR/harpoon-root.img" 2>/dev/null || stat -c%s "$OUT_DIR/harpoon-root.img")
if [ "$SZ" != "2147483648" ]; then
  echo "[guest-builder] FAIL: harpoon-root.img logical size $SZ != 2147483648" >&2
  exit 1
fi

# Run root sanitation check (must pass for release)
if ! bash "$SCRIPT_DIR/verify-root.sh" "$OUT_DIR/harpoon-root.img" 2>&1; then
  echo "[guest-builder] FAIL: root sanitation check failed — build must fail" >&2
  exit 1
fi

# Run guest verification gate (mandatory tooling: resize2fs, mgmt, modules, ordering)
if ! bash "$SCRIPT_DIR/verify-guest.sh" 2>&1; then
  echo "[guest-builder] FAIL: guest verification gate failed — build must fail" >&2
  exit 1
fi

# Hashes
if command -v shasum >/dev/null 2>&1; then
  echo "[guest-builder] hashes:" >&2
  shasum -a 256 "$OUT_DIR/Image-virt" | cut -c1-12 | xargs echo "  Image-virt" >&2 || true
  shasum -a 256 "$OUT_DIR/harpoon-initramfs.cpio.gz" | cut -c1-12 | xargs echo "  initramfs" >&2 || true
  shasum -a 256 "$OUT_DIR/harpoon-root.img" | cut -c1-12 | xargs echo "  root" >&2 || true
fi

echo "[guest-builder] done. Canonical assets at $OUT_DIR:" >&2
ls -lh "$OUT_DIR" >&2
echo "[guest-builder] produced:" >&2
echo "  $OUT_DIR/Image-virt" >&2
echo "  $OUT_DIR/harpoon-initramfs.cpio.gz" >&2
echo "  $OUT_DIR/harpoon-root.img" >&2
