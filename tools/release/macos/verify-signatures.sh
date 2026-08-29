#!/bin/bash
set -euo pipefail
# ponytail: verify Developer ID signatures for release — separate from pre-release bundle integrity
# This is the release-pipeline signature verifier (after Developer ID signing, before notarization)
APP="${1:-ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"
if [ ! -d "$APP" ]; then echo "[verify-signatures] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
echo "[verify-signatures] verifying Developer ID signatures for $APP" >&2
FAIL=0
check() { local msg="$1"; shift; if "$@"; then echo "[verify-signatures] PASS: $msg" >&2; else echo "[verify-signatures] FAIL: $msg" >&2; FAIL=1; fi; }
# Harpoon runtime must be ad-hoc or Developer ID with virtualization entitlement (pre-release ad-hoc is ok, release will be Developer ID)
check "Harpoon nested runtime valid" bash -c "codesign --verify --verbose \"$APP/Contents/Resources/harpoon/bin/harpoon\" 2>&1 | grep -q \"valid on disk\""
check "Harpoon virtualization entitlement" bash -c "codesign -d --entitlements :- \"$APP/Contents/Resources/harpoon/bin/harpoon\" 2>&1 | grep -q com.apple.security.virtualization"
# Outer app must be Developer ID Application (not ad-hoc) for release
if codesign -dv "$APP" 2>&1 | grep -q "Authority=Developer ID Application"; then
  echo "[verify-signatures] PASS: Outer app Developer ID Application" >&2
else
  echo "[verify-signatures] FAIL: Outer app not Developer ID signed (found ad-hoc or Apple Development)" >&2
  codesign -dv "$APP" 2>&1 | head -n 20 >&2 || true
  FAIL=1
fi
check "Outer app valid on disk" bash -c "codesign --verify --verbose \"$APP\" 2>&1 | grep -q \"valid on disk\""
# Notarization check (optional, after stapling)
if xcrun stapler validate "$APP" 2>&1 | grep -q "validate action worked"; then
  echo "[verify-signatures] PASS: notarization stapled" >&2
else
  echo "[verify-signatures] INFO: notarization pending (run xcrun notarytool submit)" >&2
fi
if [ $FAIL -ne 0 ]; then echo "[verify-signatures] FAIL" >&2; exit 1; fi
echo "[verify-signatures] PASS: release signatures verified" >&2
