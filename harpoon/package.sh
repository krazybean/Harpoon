#!/bin/sh
set -eu
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD="$SCRIPT_DIR/build"
DIST="$ROOT/dist"
VERSION="0.1.0"
ARCH="$(uname -m)"
if [ "$ARCH" != "arm64" ]; then echo "[package] unsupported arch $ARCH (only arm64)" >&2; exit 1; fi
STAGE="$DIST/harpoon-$VERSION-darwin-$ARCH"
echo "[package] building harpoon..." >&2
bash "$SCRIPT_DIR/build.sh" 2>&1 | tail -n 5
echo "[package] verifying artifacts..." >&2
# Guest verification gate — catches missing resize2fs/mgmt before packaging
if [ -f "$ROOT/tools/guest-builder/verify-guest.sh" ]; then
  bash "$ROOT/tools/guest-builder/verify-guest.sh" 2>&1 || { echo "[package] FAIL: guest verification gate — cannot package" >&2; exit 1; }
fi
[ -f "$BUILD/harpoon" ] || { echo "[package] binary missing" >&2; exit 1; }
[ -f "$ROOT/assets/guest/Image-virt" ] || { echo "[package] kernel missing assets/guest/Image-virt (run: bash tools/guest-builder/build.sh)" >&2; exit 1; }
[ -f "$ROOT/assets/guest/harpoon-initramfs.cpio.gz" ] || { echo "[package] initramfs missing assets/guest/harpoon-initramfs.cpio.gz (run: bash tools/guest-builder/build.sh)" >&2; exit 1; }
[ -f "$ROOT/assets/guest/harpoon-root.img" ] || { echo "[package] root disk missing assets/guest/harpoon-root.img (run: bash tools/guest-builder/build.sh)" >&2; exit 1; }
file "$BUILD/harpoon" | grep -q "arm64" || { echo "[package] not arm64" >&2; exit 1; }
codesign -d --entitlements :- "$BUILD/harpoon" 2>&1 | grep -q "com.apple.security.virtualization" || { echo "[package] entitlement missing" >&2; exit 1; }
codesign --verify --verbose "$BUILD/harpoon" 2>&1 | grep -q "valid on disk" || { echo "[package] codesign invalid" >&2; exit 1; }
echo "[package] staging to $STAGE..." >&2
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/lib/harpoon" "$STAGE/share/doc/harpoon"
cp "$BUILD/harpoon" "$STAGE/bin/harpoon"
chmod +x "$STAGE/bin/harpoon"
cp "$ROOT/assets/guest/Image-virt" "$STAGE/lib/harpoon/Image-virt"
cp "$ROOT/assets/guest/harpoon-initramfs.cpio.gz" "$STAGE/lib/harpoon/harpoon-initramfs.cpio.gz"
# root template: clone-aware (APFS cp -c preserves sparse; fallback to ditto/cp)
if cp -c "$ROOT/assets/guest/harpoon-root.img" "$STAGE/lib/harpoon/harpoon-root.img" 2>/dev/null; then :; elif ditto "$ROOT/assets/guest/harpoon-root.img" "$STAGE/lib/harpoon/harpoon-root.img" 2>/dev/null; then :; else cp "$ROOT/assets/guest/harpoon-root.img" "$STAGE/lib/harpoon/harpoon-root.img"; fi
# docs
cp "$ROOT/README.md" "$STAGE/share/doc/harpoon/" 2>/dev/null || true
cp "$ROOT/docs/installation.md" "$STAGE/share/doc/harpoon/" 2>/dev/null || true
# install scripts (will be created)
if [ -f "$ROOT/harpoon/install.sh" ]; then cp "$ROOT/harpoon/install.sh" "$STAGE/install.sh"; chmod +x "$STAGE/install.sh"; fi
if [ -f "$ROOT/harpoon/uninstall.sh" ]; then cp "$ROOT/harpoon/uninstall.sh" "$STAGE/uninstall.sh"; chmod +x "$STAGE/uninstall.sh"; fi
# verify staged binary still signed after copy
codesign --verify --verbose "$STAGE/bin/harpoon" 2>&1 | grep -q "valid on disk" || { echo "[package] staged binary not valid" >&2; exit 1; }
# sizes
echo "[package] sizes:" >&2
ls -lh "$STAGE/bin/harpoon" "$STAGE/lib/harpoon/"* 2>&1 | tail -n 10 >&2
du -h "$STAGE/lib/harpoon/harpoon-root.img" 2>&1 | tail -n 1 >&2
echo "[package] staged at $STAGE" >&2
# archive
ARCHIVE="$DIST/harpoon-$VERSION-darwin-$ARCH.tar.gz"
echo "[package] creating archive $ARCHIVE..." >&2
tar -czf "$ARCHIVE" -C "$DIST" "harpoon-$VERSION-darwin-$ARCH"
shasum -a 256 "$ARCHIVE" > "$ARCHIVE.sha256"
cat "$ARCHIVE.sha256" >&2
ls -lh "$ARCHIVE" "$ARCHIVE.sha256" >&2
echo "[package] done" >&2
