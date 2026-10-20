#!/bin/sh
set -eu
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SRC_TAURI="$SCRIPT_DIR"
BUILD="$REPO_ROOT/harpoon/build/harpoon"
KERNEL="$REPO_ROOT/assets/guest/Image-virt"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
ROOTIMG="$REPO_ROOT/assets/guest/harpoon-root.img"
ASSET_MANIFEST="$REPO_ROOT/assets/guest/guest-assets.manifest"

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
cp -p "$ASSET_MANIFEST" "$HARPOON_LIB_DIR/guest-assets.manifest"
# APFS clone-aware for sparse root image
if cp -c "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img" 2>/dev/null; then :;
elif ditto "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img" 2>/dev/null; then :;
else cp -p "$ROOTIMG" "$HARPOON_LIB_DIR/harpoon-root.img"
fi

# Universal bundles also carry the x86_64 guest for Intel Macs in lib/harpoon/x86_64.
if [ "${HARPOON_ARCH:-arm64}" = "universal" ]; then
  X86_SRC="$REPO_ROOT/assets/guest/x86_64"
  X86_DST="$HARPOON_LIB_DIR/x86_64"
  for f in Image-virt harpoon-initramfs.cpio.gz guest-assets.manifest harpoon-root.img; do
    [ -f "$X86_SRC/$f" ] || { echo "[prepare-bundle] FAIL: missing x86_64 guest asset $X86_SRC/$f (HARPOON_GUEST_ARCH=x86_64 bash tools/guest-builder/build.sh)" >&2; exit 1; }
  done
  mkdir -p "$X86_DST"
  echo "[prepare-bundle] copying x86_64 kernel/initramfs/root..." >&2
  cp -p "$X86_SRC/Image-virt" "$X86_SRC/harpoon-initramfs.cpio.gz" "$X86_SRC/guest-assets.manifest" "$X86_DST/"
  if cp -c "$X86_SRC/harpoon-root.img" "$X86_DST/harpoon-root.img" 2>/dev/null; then :;
  elif ditto "$X86_SRC/harpoon-root.img" "$X86_DST/harpoon-root.img" 2>/dev/null; then :;
  else cp -p "$X86_SRC/harpoon-root.img" "$X86_DST/harpoon-root.img"
  fi
fi

# verify
ls -lh "$HARPOON_BIN_DIR/harpoon" "$HARPOON_LIB_DIR/"* 2>&1 | tail -n 10 >&2
echo "[prepare-bundle] bundle-resources ready at $BUNDLE_RES" >&2
# hashes
shasum -a 256 "$HARPOON_BIN_DIR/harpoon" | head -c 12 | xargs echo "[prepare-bundle] harpoon" >&2 || true
shasum -a 256 "$HARPOON_LIB_DIR/Image-virt" | head -c 12 | xargs echo "[prepare-bundle] kernel" >&2 || true
