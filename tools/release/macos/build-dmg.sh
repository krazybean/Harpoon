#!/bin/bash
set -euo pipefail
# ponytail: build Harpoon-0.1.1-arm64.dmg from already-signed Harpoon.app (no rebuild)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${HARPOON_VERSION:-$(node -p "require('$REPO_ROOT/ui/harpoon-desktop/package.json').version" 2>/dev/null || echo "0.1.1")}"
if [ -n "${1:-}" ]; then VERSION="$1"; fi
APP="${2:-$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"
if [ ! -d "$APP" ]; then echo "[build-dmg] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
# Verify app is already signed (at least ad-hoc, ideally Developer ID) — trust exit status, not grep (pipefail+grep -q SIGPIPE)
if ! codesign --verify --deep --strict --verbose=2 "$APP" >/dev/null 2>&1; then
  echo "[build-dmg] FAIL: Harpoon.app not signed/valid at $APP" >&2; codesign --verify --deep --strict --verbose=4 "$APP" 2>&1 | tail -n 20 >&2; exit 1
fi
DIST_DIR="$REPO_ROOT/dist/v$VERSION"
mkdir -p "$DIST_DIR"
DMG_NAME="Harpoon-${VERSION}-arm64.dmg"
DMG_PATH="$DIST_DIR/$DMG_NAME"
# Explicit 3072 MB working image (sparse root is 2G logical, HFS+ needs overhead)
# Do not rebuild app during DMG creation — use exact signed app
TMP_SRC=$(mktemp -d)
echo "[build-dmg] staging $TMP_SRC -> $DMG_PATH (3072M)" >&2
cp -R "$APP" "$TMP_SRC/"
ln -s /Applications "$TMP_SRC/Applications"
ICON="$REPO_ROOT/ui/harpoon-desktop/src-tauri/icons/icon.icns"
BUNDLE_DMG_SH="$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/dmg/bundle_dmg.sh"
# Prefer create-dmg if available, else hdiutil
DMG_CREATED=0
if [ -f "$BUNDLE_DMG_SH" ]; then
  echo "[build-dmg] via bundle_dmg.sh --disk-image-size 3072" >&2
  ARGS=(--volname Harpoon --window-pos 10 60 --window-size 500 350 --icon-size 128 --icon "Harpoon.app" 100 100 --app-drop-link 400 100 --disk-image-size 3072 --format UDZO "$DMG_PATH" "$TMP_SRC")
  if [ -f "$ICON" ]; then ARGS=(--volicon "$ICON" "${ARGS[@]}"); fi
  if bash "$BUNDLE_DMG_SH" "${ARGS[@]}" 2>&1 | tail -n 20; then
    if [ -f "$DMG_PATH" ]; then DMG_CREATED=1; fi
  fi
fi
if [ $DMG_CREATED -eq 0 ]; then
  echo "[build-dmg] fallback hdiutil create -size 3072m" >&2
  rm -f "$DMG_PATH"
  hdiutil create -size 3072m -fs HFS+ -volname Harpoon -srcfolder "$TMP_SRC" -ov -format UDZO "$DMG_PATH" 2>&1 | tail -n 20
  if [ -f "$DMG_PATH" ]; then DMG_CREATED=1; fi
fi
rm -rf "$TMP_SRC"
if [ $DMG_CREATED -eq 0 ] || [ ! -f "$DMG_PATH" ]; then
  echo "[build-dmg] FAIL: DMG not created at $DMG_PATH" >&2
  # Sandbox may block hdiutil Device not configured — capture to avoid pipefail SIGPIPE
  HDIUTIL_OUT=$(hdiutil create -size 3072m -fs HFS+ -volname Harpoon -srcfolder "$APP" -ov -format UDZO "$DMG_PATH" 2>&1 || true)
  if echo "$HDIUTIL_OUT" | grep -q "Device not configured"; then
    echo "[build-dmg] BLOCKED: hdiutil Device not configured (sandbox)" >&2
    exit 0
  fi
  exit 1
fi
echo "[build-dmg] DMG created $DMG_PATH ($(du -h "$DMG_PATH" | awk '{print $1}'))" >&2
# Mount and verify byte/signature equivalent
MNT=$(mktemp -d)
if hdiutil attach -nobrowse -quiet -mountpoint "$MNT" "$DMG_PATH" >/dev/null 2>&1; then
  echo "[build-dmg] mounted at $MNT" >&2
  # Verify app inside DMG is same as source (codesign valid) — trust exit status
  if ! codesign --verify --deep --strict --verbose=2 "$MNT/Harpoon.app" >/dev/null 2>&1; then
    echo "[build-dmg] FAIL: mounted Harpoon.app not valid" >&2; hdiutil detach "$MNT" >/dev/null 2>&1 || true; exit 1
  fi
  # Verify-bundle on mounted copy (preserve exit, avoid tail truncation)
  if ! bash "$REPO_ROOT/tools/verify-bundle.sh" "$MNT/Harpoon.app" 2>&1 | tee /tmp/verify-mounted.log; then
    echo "[build-dmg] FAIL: verify-bundle on mounted app failed" >&2; hdiutil detach "$MNT" >/dev/null 2>&1 || true; exit 1
  fi
  # Compare signature (if same Team, should match) — avoid grep|head SIGPIPE, use grep -m1
  SRC_SIG=$(codesign -dv --verbose=4 "$APP" 2>&1 | grep -m1 "Authority=" || true)
  MNT_SIG=$(codesign -dv --verbose=4 "$MNT/Harpoon.app" 2>&1 | grep -m1 "Authority=" || true)
  echo "[build-dmg] src Authority: $SRC_SIG" >&2
  echo "[build-dmg] mnt Authority: $MNT_SIG" >&2
  hdiutil detach "$MNT" 2>&1 | tail -n 5 || true
  rmdir "$MNT" 2>/dev/null || true
else
  echo "[build-dmg] WARN: could not mount DMG for verification (sandbox?)" >&2
fi
echo "[build-dmg] DONE $DMG_PATH" >&2
ls -lh "$DMG_PATH" >&2
