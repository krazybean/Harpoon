#!/bin/bash
set -euo pipefail
# ponytail: notarize DMG via loom-notary profile (do not create/delete Keychain item)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${HARPOON_VERSION:-$(node -p "require('$REPO_ROOT/ui/harpoon-desktop/package.json').version" 2>/dev/null || echo "0.1.1")}"
if [ -n "${1:-}" ]; then
  # allow version or DMG path
  if [[ "$1" == *.dmg ]]; then DMG="$1"; else VERSION="$1"; DMG="${2:-}"; fi
fi
DMG="${DMG:-$REPO_ROOT/dist/v$VERSION/Harpoon-${VERSION}-arm64.dmg}"
if [ ! -f "$DMG" ]; then echo "[notarize] FAIL: DMG not found at $DMG" >&2; exit 1; fi
# Check keychain profile exists (do not create)
if ! xcrun notarytool history --keychain-profile "loom-notary" 2>&1 | head -n 5 | grep -q .; then
  echo "[notarize] checking loom-notary profile..." >&2
  if ! security find-generic-password -s "notarytool:loom-notary" 2>&1 | head | grep -q .; then
    echo "[notarize] WARN: loom-notary profile may not be configured, trying anyway" >&2
  fi
fi
echo "[notarize] submitting $DMG --keychain-profile loom-notary --wait" >&2
# Capture submission ID and status
LOG_TMP=$(mktemp)
DIST_DIR_LOCAL="$(dirname "$DMG")"
set +e
xcrun notarytool submit "$DMG" --keychain-profile "loom-notary" --wait 2>&1 | tee "$LOG_TMP" | tee "$DIST_DIR_LOCAL/notarize.log"
STATUS=${PIPESTATUS[0]}
set -e
cat "$LOG_TMP" >&2
cp "$LOG_TMP" "$DIST_DIR_LOCAL/notarize.log" 2>/dev/null || true
if [ $STATUS -ne 0 ]; then
  echo "[notarize] FAIL: notarytool submit exit $STATUS" >&2
  # Try to fetch log via submission ID if present
  SUBID=$(grep -E "id: [a-f0-9-]{36}" "$LOG_TMP" | head -n1 | grep -oE "[a-f0-9-]{36}" || true)
  if [ -n "$SUBID" ]; then
    echo "[notarize] fetching log for $SUBID..." >&2
    xcrun notarytool log "$SUBID" --keychain-profile "loom-notary" 2>&1 | tail -n 100 >&2 || true
  fi
  rm -f "$LOG_TMP"
  exit $STATUS
fi
# Check for Accepted
if grep -q "status: Accepted" "$LOG_TMP"; then
  echo "[notarize] PASS Accepted" >&2
else
  echo "[notarize] FAIL: not Accepted" >&2
  cat "$LOG_TMP" >&2
  rm -f "$LOG_TMP"
  exit 1
fi
SUBID=$(grep -E "id: [a-f0-9-]{36}" "$LOG_TMP" | head -n1 | grep -oE "[a-f0-9-]{36}" || echo "unknown")
echo "[notarize] submission ID $SUBID" >&2
rm -f "$LOG_TMP"
# Staple
echo "[notarize] stapling $DMG..." >&2
xcrun stapler staple "$DMG" 2>&1 | tail -n 10
echo "[notarize] validating staple..." >&2
xcrun stapler validate "$DMG" 2>&1 | tail -n 10
# Also validate app inside DMG if mounted
echo "[notarize] DONE" >&2
