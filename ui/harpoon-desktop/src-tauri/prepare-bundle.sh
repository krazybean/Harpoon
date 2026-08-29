#!/bin/sh
set -eu
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SRC_TAURI="$SCRIPT_DIR"
BUILD="$REPO_ROOT/harpoon/build/harpoon"
KERNEL="$REPO_ROOT/assets/guest/Image-virt"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
ROOTIMG="$REPO_ROOT/assets/guest/harpoon-root.img"

# ponytail: ensure runtime built before bundling
if [ ! -f "$BUILD" ]; then
  echo "[prepare-bundle] harpoon binary missing, building..." >&2
  bash "$REPO_ROOT/harpoon/build.sh" 2>&1 | tail -n 10
fi

BUNDLE_RES="$SRC_TAURI/bundle-resources"
HARPOON_BIN_DIR="$BUNDLE_RES/harpoon/bin"
HARPOON_LIB_DIR="$BUNDLE_RES/harpoon/lib/harpoon"

mkdir -p "$HARPOON_BIN_DIR" "$HARPOON_LIB_DIR"

echo "[prepare-bundle] copying harpoon binary..." >&2
cp -p "$BUILD" "$HARPOON_BIN_DIR/harpoon"
chmod +x "$HARPOON_BIN_DIR/harpoon"

echo "[prepare-bundle] copying kernel/initramfs/root..." >&2
cp -p "$KERNEL" "$HARPOON_LIB_DIR/Image-virt"
cp -p "$INITRAMFS" "$HARPOON_LIB_DIR/harpoon-initramfs.cpio.gz"
# APFS clone-aware for sparse root image
if cp -c "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img" 2>/dev/null; then :;
elif ditto "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img" 2>/dev/null; then :;
else cp -p "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img"
fi

# verify
ls -lh "$HARPOON_BIN_DIR/harpoon" "$HARPOON_LIB_DIR/"* 2>&1 | tail -n 10 >&2
echo "[prepare-bundle] bundle-resources ready at $BUNDLE_RES" >&2
# hashes
shasum -a 256 "$HARPOON_BIN_DIR/harpoon" | head -c 12 | xargs echo "[prepare-bundle] harpoon" >&2 || true
shasum -a 256 "$HARPOON_LIB_DIR/Image-virt" | head -c 12 | xargs echo "[prepare-bundle] kernel" >&2 || true
