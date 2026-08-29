#!/bin/bash
set -euo pipefail
# ponytail: sign Harpoon.app inside-out with Developer ID (not ad-hoc, not --deep borg)
# Requires HARPOON_SIGN_IDENTITY="Developer ID Application: Name (TEAMID)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
APP="${1:-$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"
IDENTITY="${HARPOON_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ]; then echo "[sign-app] FAIL: HARPOON_SIGN_IDENTITY not set (Developer ID Application)" >&2; echo "  export HARPOON_SIGN_IDENTITY=\"Developer ID Application: Your Name (TEAMID)\"" >&2; security find-identity -v -p codesigning 2>&1 | head -n 20 >&2; exit 1; fi
# Validate identity exists and is Developer ID Application
if ! security find-identity -v -p codesigning 2>&1 | grep -F "$IDENTITY" | grep -q "Developer ID Application"; then
  echo "[sign-app] FAIL: identity not found or not Developer ID Application: $IDENTITY" >&2
  security find-identity -v -p codesigning 2>&1 | head -n 30 >&2
  exit 1
fi
if [ ! -d "$APP" ]; then echo "[sign-app] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
INNER="$APP/Contents/Resources/harpoon/bin/harpoon"
ENTITLEMENTS="$REPO_ROOT/harpoon/entitlements.plist"
if [ ! -f "$INNER" ]; then echo "[sign-app] FAIL: nested harpoon not found at $INNER" >&2; exit 1; fi
if [ ! -f "$ENTITLEMENTS" ]; then echo "[sign-app] FAIL: entitlements not found at $ENTITLEMENTS" >&2; exit 1; fi
echo "[sign-app] signing Developer ID inside-out: $IDENTITY" >&2
echo "[sign-app] app: $APP" >&2
# Enumerate nested executable code (no --deep)
# 1. Nested Swift runtime with entitlements + hardened runtime + timestamp
echo "[sign-app] signing nested harpoon with virtualization entitlement..." >&2
codesign --force --sign "$IDENTITY" --entitlements "$ENTITLEMENTS" --options runtime --timestamp --verbose "$INNER"
if [ $? -ne 0 ]; then echo "[sign-app] FAIL: nested sign failed" >&2; exit 1; fi
# 2. Frameworks if present (Swift libs, etc)
if [ -d "$APP/Contents/Frameworks" ]; then
  for fw in "$APP/Contents/Frameworks"/*; do
    [ -e "$fw" ] || continue
    echo "[sign-app] signing Framework $fw..." >&2
    codesign --force --sign "$IDENTITY" --options runtime --timestamp --verbose "$fw"
    if [ $? -ne 0 ]; then echo "[sign-app] FAIL: Framework sign failed for $fw" >&2; exit 1; fi
  done
fi
# 3. Outer executable (Tauri) with hardened runtime + timestamp (no entitlements)
OUTER="$APP/Contents/MacOS/harpoon-desktop"
if [ -f "$OUTER" ]; then
  echo "[sign-app] signing outer harpoon-desktop with hardened runtime..." >&2
  codesign --force --sign "$IDENTITY" --options runtime --timestamp --verbose "$OUTER"
  if [ $? -ne 0 ]; then echo "[sign-app] FAIL: outer executable sign failed" >&2; exit 1; fi
fi
# 4. Outer app bundle last (without --deep, but now all nested is already signed)
echo "[sign-app] signing outer Harpoon.app bundle..." >&2
codesign --force --sign "$IDENTITY" --options runtime --timestamp --verbose "$APP"
if [ $? -ne 0 ]; then echo "[sign-app] FAIL: bundle sign failed" >&2; exit 1; fi
echo "[sign-app] verifying..." >&2
codesign --verify --deep --strict --verbose=2 "$APP"
status=$?
if [ $status -ne 0 ]; then echo "[sign-app] FAIL verify deep strict $status" >&2; exit $status; fi
# Entitlements check: inner must have virtualization, outer must not
if ! codesign -d --entitlements :- "$INNER" 2>&1 | grep -q "com.apple.security.virtualization"; then echo "[sign-app] FAIL inner missing virtualization" >&2; exit 1; fi
if codesign -d --entitlements :- "$OUTER" 2>&1 | grep -q "com.apple.security.virtualization"; then echo "[sign-app] FAIL outer should not have virtualization" >&2; exit 1; fi
if [ ! -f "$APP/Contents/_CodeSignature/CodeResources" ]; then echo "[sign-app] FAIL CodeResources missing" >&2; exit 1; fi
echo "[sign-app] PASS Developer ID signed inside-out" >&2
codesign -dv --verbose=4 "$APP" 2>&1 | grep -E "Authority|TeamIdentifier|Timestamp" | head -n 10 >&2
