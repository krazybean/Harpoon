#!/bin/bash
set -euo pipefail
# ponytail: pack the built guests (aarch64 + x86_64) into one tarball for CI
# (published by tools/release/macos/publish-guest-assets.sh for .github/workflows/universal-dmg.yml).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ASSETS="$REPO_ROOT/assets/guest"
OUT="${1:-$REPO_ROOT/dist/harpoon-guest-assets.tar.gz}"
FILES=()
for dir in "" "x86_64/"; do
  for f in Image-virt harpoon-initramfs.cpio.gz guest-assets.manifest harpoon-root.img harpoon-root.img.manifest; do
    [ -f "$ASSETS/$dir$f" ] || { echo "[pack-assets] FAIL: missing $ASSETS/$dir$f" >&2; exit 1; }
    FILES+=("$dir$f")
  done
done
mkdir -p "$(dirname "$OUT")"
tar -czf "$OUT" -C "$ASSETS" "${FILES[@]}"
echo "[pack-assets] $OUT" >&2
shasum -a 256 "$OUT"
