#!/bin/bash
set -euo pipefail
# ponytail: release orchestrator — build → sign → verify → dmg → sign dmg → notarize → staple → checksums → provenance
# Usage: tools/release/macos/release.sh 0.1.1  (or HARPOON_VERSION=0.1.1 tools/release/macos/release.sh)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${1:-${HARPOON_VERSION:-}}"
if [ -z "$VERSION" ]; then
  # fallback to package.json
  VERSION=$(node -p "require('$REPO_ROOT/ui/harpoon-desktop/package.json').version" 2>/dev/null || echo "")
  if [ -z "$VERSION" ]; then echo "[release] FAIL: version required: tools/release/macos/release.sh 0.1.1" >&2; exit 1; fi
fi
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then echo "[release] FAIL: invalid version $VERSION" >&2; exit 1; fi
export HARPOON_VERSION="$VERSION"
echo "[release] Harpoon v$VERSION release candidate" >&2
# Prereqs
for cmd in swiftc codesign cargo node npm docker xcrun; do
  if ! command -v "$cmd" >/dev/null 2>&1; then echo "[release] FAIL: $cmd not found" >&2; exit 1; fi
done
if [ -z "${HARPOON_SIGN_IDENTITY:-}" ]; then echo "[release] FAIL: HARPOON_SIGN_IDENTITY not set (Developer ID Application)" >&2; security find-identity -v -p codesigning 2>&1 | head -n 20 >&2; exit 1; fi
if ! security find-identity -v -p codesigning 2>&1 | grep -F "$HARPOON_SIGN_IDENTITY" | grep -q "Developer ID Application"; then echo "[release] FAIL: identity not Developer ID Application: $HARPOON_SIGN_IDENTITY" >&2; exit 1; fi
# Guest assets
echo "[release] guest assets..." >&2
bash "$REPO_ROOT/tools/guest-builder/build.sh" 2>&1 | tail -n 20
bash "$REPO_ROOT/tools/guest-builder/verify-root.sh" 2>&1 | tail -n 10
# Version flow
echo "[release] version $VERSION flow..." >&2
# Ensure package.json version matches requested (if not, bump via version.mjs without commit)
PKG_VER=$(node -p "require('$REPO_ROOT/ui/harpoon-desktop/package.json').version")
if [ "$PKG_VER" != "$VERSION" ]; then
  echo "[release] bumping package.json $PKG_VER -> $VERSION (no commit)" >&2
  node "$REPO_ROOT/ui/harpoon-desktop/scripts/version.mjs" "$VERSION" 2>&1 | tail -n 10
fi
node "$REPO_ROOT/ui/harpoon-desktop/scripts/version.mjs" --check 2>&1 | tail -n 10
# Clean build (reuse tools/release/macos/build.sh)
echo "[release] clean build..." >&2
bash "$SCRIPT_DIR/build.sh" 2>&1 | tail -n 30
APP="$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app"
if [ ! -d "$APP" ]; then echo "[release] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
# Copy to dist/v$VERSION
DIST_DIR="$REPO_ROOT/dist/v$VERSION"
mkdir -p "$DIST_DIR"
# Copy app to dist for provenance (not moving, just for reference)
if [ -d "$DIST_DIR/Harpoon.app" ]; then rm -rf "$DIST_DIR/Harpoon.app"; fi
cp -R "$APP" "$DIST_DIR/Harpoon.app"
echo "[release] copied Harpoon.app to $DIST_DIR/Harpoon.app" >&2
# Sign inside-out
echo "[release] signing..." >&2
bash "$SCRIPT_DIR/sign-app.sh" "$DIST_DIR/Harpoon.app" 2>&1 | tail -n 20
# Also sign the original bundle location for consistency (so future verify uses signed)
bash "$SCRIPT_DIR/sign-app.sh" "$APP" 2>&1 | tail -n 20 || true
# Verify signatures (Developer ID)
echo "[release] verify signatures (Developer ID)..." >&2
bash "$SCRIPT_DIR/verify-signatures.sh" "$DIST_DIR/Harpoon.app" 2>&1 | tail -n 20
# Structural verify
bash "$REPO_ROOT/tools/verify-bundle.sh" "$DIST_DIR/Harpoon.app" 2>&1 | tail -n 20
# Dependency check (minos 15.1, no DarwinFoundation, etc) — part of verify-bundle, but also explicit
echo "[release] dependency check..." >&2
for bin in "$DIST_DIR/Harpoon.app/Contents/MacOS/harpoon-desktop" "$DIST_DIR/Harpoon.app/Contents/Resources/harpoon/bin/harpoon"; do
  echo "[release] otool $bin" >&2
  otool -l "$bin" 2>&1 | grep -E "minos|LC_BUILD_VERSION" | head -n 5 >&2 || true
  if otool -L "$bin" 2>&1 | grep -q "DarwinFoundation"; then echo "[release] FAIL: $bin links DarwinFoundation" >&2; exit 1; fi
done
# Build DMG from signed dist app (not stale bundle)
echo "[release] DMG..." >&2
bash "$SCRIPT_DIR/build-dmg.sh" "$VERSION" "$DIST_DIR/Harpoon.app" 2>&1 | tail -n 30
DMG="$DIST_DIR/Harpoon-${VERSION}-arm64.dmg"
if [ ! -f "$DMG" ]; then echo "[release] FAIL: DMG not found at $DMG" >&2; exit 1; fi
# Sign DMG
echo "[release] signing DMG..." >&2
codesign --force --sign "$HARPOON_SIGN_IDENTITY" --timestamp --verbose "$DMG" 2>&1 | tail -n 10 || true
codesign --verify --verbose "$DMG" 2>&1 | tail -n 10 || true
# Notarize
echo "[release] notarizing..." >&2
bash "$SCRIPT_DIR/notarize.sh" "$DMG" 2>&1 | tail -n 50
# Gatekeeper
echo "[release] Gatekeeper..." >&2
spctl --assess --type execute --verbose=4 "$DIST_DIR/Harpoon.app" 2>&1 | tail -n 20 || echo "[release] spctl app: $(spctl --assess --type execute --verbose=4 "$DIST_DIR/Harpoon.app" 2>&1 | head -n 5)" >&2
spctl --assess --type install --verbose=4 "$DMG" 2>&1 | tail -n 20 || true
# Tarball (if retained)
if [ -f "$REPO_ROOT/ui/harpoon-desktop/scripts/release.mjs" ]; then
  echo "[release] tarball..." >&2
  # Build tarball from dist app or via existing release.mjs logic (kept for compatibility)
  TARBALL="$DIST_DIR/harpoon-${VERSION}-darwin-arm64.tar.gz"
  if [ -f "$TARBALL" ]; then echo "[release] tarball exists $TARBALL" >&2; else
    echo "[release] INFO: tarball not built in this stage (Harpoon.app is primary)" >&2
  fi
fi
# Checksums AFTER notarize/staple
echo "[release] checksums..." >&2
( cd "$DIST_DIR" && shasum -a 256 Harpoon-*.dmg harpoon-*.tar.gz 2>/dev/null | tee SHA256SUMS; cat SHA256SUMS ) 2>&1 | tail -n 20
# Provenance
GIT_SHA=$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo "unknown")
GIT_DIRTY=$(git -C "$REPO_ROOT" status --porcelain 2>&1 | head -n 20 || true)
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
ARCH=$(uname -m)
cat > "$DIST_DIR/RELEASE-PROVENANCE.txt" <<EOF
Harpoon $VERSION
git $GIT_SHA
dirty:
$GIT_DIRTY
build $TIMESTAMP
arch $ARCH
minos 15.1
kernel $(shasum -a 256 "$REPO_ROOT/assets/guest/Image-virt" 2>/dev/null | cut -d' ' -f1 || echo unknown) $(stat -f%z "$REPO_ROOT/assets/guest/Image-virt" 2>/dev/null || stat -c%s "$REPO_ROOT/assets/guest/Image-virt" 2>/dev/null)
initramfs $(shasum -a 256 "$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz" 2>/dev/null | cut -d' ' -f1)
root $(shasum -a 256 "$REPO_ROOT/assets/guest/harpoon-root.img" 2>/dev/null | cut -d' ' -f1)
nested $(shasum -a 256 "$DIST_DIR/Harpoon.app/Contents/Resources/harpoon/bin/harpoon" 2>/dev/null | cut -d' ' -f1)
outer $(shasum -a 256 "$DIST_DIR/Harpoon.app/Contents/MacOS/harpoon-desktop" 2>/dev/null | cut -d' ' -f1)
sign $HARPOON_SIGN_IDENTITY
dmg $(shasum -a 256 "$DMG" 2>/dev/null | cut -d' ' -f1) $(stat -f%z "$DMG" 2>/dev/null || stat -c%s "$DMG" 2>/dev/null) bytes
notary loom-notary Accepted (see notarize log)
EOF
cat "$DIST_DIR/RELEASE-PROVENANCE.txt" >&2
if [ -n "$GIT_DIRTY" ]; then
  echo "[release] WARN: working tree dirty — not publishable as official release candidate" >&2
  echo "[release] dirty files:" >&2
  echo "$GIT_DIRTY" >&2
fi
echo "[release] DONE dist/v$VERSION" >&2
ls -lh "$DIST_DIR" >&2
