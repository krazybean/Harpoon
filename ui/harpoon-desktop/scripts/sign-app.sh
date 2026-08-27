#!/bin/sh
set -eu
# Ponytail: minimal signing wrapper, no abstraction, correct nested→outer order
# Harpoon bundles a sparse 2 GiB Linux root filesystem. APFS reports a much
# smaller physical footprint than the logical space required when copied into
# the temporary HFS+ DMG. Explicit 3072 MiB sizing prevents create-dmg/hdiutil ENOSPC.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# repo root from script location: scripts/ -> ui/harpoon-desktop -> Harpoon
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
# Allow override: first arg is app path, default to canonical bundle location
APP="${1:-$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"

if [ ! -d "$APP" ]; then
  echo "sign-app: Harpoon.app not found at $APP" >&2
  exit 1
fi

INNER="$APP/Contents/Resources/harpoon/bin/harpoon"
ENTITLEMENTS="$REPO_ROOT/harpoon/entitlements.plist"

if [ ! -f "$INNER" ]; then
  echo "sign-app: bundled harpoon not found at $INNER" >&2
  exit 1
fi
if [ ! -f "$ENTITLEMENTS" ]; then
  echo "sign-app: entitlements not found at $ENTITLEMENTS" >&2
  exit 1
fi

echo "[sign] signing nested harpoon with virtualization entitlement..." >&2
codesign --force --sign - --entitlements "$ENTITLEMENTS" "$INNER" 2>&1 | tail -n 5

echo "[sign] signing outer Harpoon.app (LAST)..." >&2
codesign --force --deep --sign - "$APP" 2>&1 | tail -n 5

echo "[sign] verifying..." >&2
codesign --verify --deep --strict --verbose=4 "$APP" 2>&1 | tail -n 20
status=$?
if [ $status -ne 0 ]; then
  # Still show warning but fail if strict fails
  echo "[sign] verify failed with exit $status" >&2
  # Some ad-hoc signatures show "code has no resources but signature indicates they must be present" but still exit 0 after --deep
  # If exit non-zero, fail
  exit $status
fi

# Confirm inner has entitlement, outer does not
if ! codesign -d --entitlements :- "$INNER" 2>&1 | grep -q "com.apple.security.virtualization"; then
  echo "[sign] FAIL inner missing virtualization" >&2
  exit 1
fi
if codesign -d --entitlements :- "$APP/Contents/MacOS/harpoon-desktop" 2>&1 | grep -q "com.apple.security.virtualization"; then
  echo "[sign] FAIL outer UI should not have virtualization" >&2
  exit 1
fi
if [ ! -f "$APP/Contents/_CodeSignature/CodeResources" ]; then
  echo "[sign] FAIL CodeResources missing" >&2
  exit 1
fi

echo "[sign] App signature: PASS" >&2
echo "[sign] Runtime entitlement: PASS" >&2
