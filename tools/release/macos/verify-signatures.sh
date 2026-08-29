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
# Timestamp checks — require secure timestamp for Developer ID release (capture to avoid pipefail+SIGPIPE)
check "Harpoon nested has secure timestamp" bash -c "OUT=\$(codesign -dv --verbose=4 \"$APP/Contents/Resources/harpoon/bin/harpoon\" 2>&1); echo \"\$OUT\" | grep -q \"Timestamp=\""
check "Harpoon outer executable has secure timestamp" bash -c "OUT=\$(codesign -dv --verbose=4 \"$APP/Contents/MacOS/harpoon-desktop\" 2>&1); echo \"\$OUT\" | grep -q \"Timestamp=\""
check "Harpoon.app bundle has secure timestamp" bash -c "OUT=\$(codesign -dv --verbose=4 \"$APP\" 2>&1); echo \"\$OUT\" | grep -q \"Timestamp=\""
# Ad-hoc check — must NOT be ad-hoc (flags should not contain adhoc) — capture to avoid pipefail SIGPIPE
NESTED_FLAGS=$(codesign -dv --verbose=4 "$APP/Contents/Resources/harpoon/bin/harpoon" 2>&1)
if echo "$NESTED_FLAGS" | grep -q "flags=.*adhoc"; then
  echo "[verify-signatures] FAIL: nested is ad-hoc (missing Developer ID)" >&2; FAIL=1
else
  echo "[verify-signatures] PASS: nested not ad-hoc" >&2
fi
OUTER_EXE_FLAGS=$(codesign -dv --verbose=4 "$APP/Contents/MacOS/harpoon-desktop" 2>&1)
if echo "$OUTER_EXE_FLAGS" | grep -q "flags=.*adhoc"; then
  echo "[verify-signatures] FAIL: outer executable is ad-hoc" >&2; FAIL=1
else
  echo "[verify-signatures] PASS: outer executable not ad-hoc" >&2
fi
BUNDLE_FLAGS=$(codesign -dv --verbose=4 "$APP" 2>&1)
if echo "$BUNDLE_FLAGS" | grep -q "flags=.*adhoc"; then
  echo "[verify-signatures] FAIL: bundle is ad-hoc" >&2; FAIL=1
else
  echo "[verify-signatures] PASS: bundle not ad-hoc" >&2
fi
# Outer app bundle must be Developer ID Application — use --verbose=4 to expose Authority chain (plain -dv hides it); capture to avoid pipefail SIGPIPE
OUTER_BUNDLE_OUT=$(codesign -dv --verbose=4 "$APP" 2>&1)
if echo "$OUTER_BUNDLE_OUT" | grep -q "Authority=Developer ID Application"; then
  echo "[verify-signatures] PASS: Outer app Developer ID Application" >&2
else
  echo "[verify-signatures] FAIL: Outer app not Developer ID signed (found ad-hoc or Apple Development)" >&2
  echo "$OUTER_BUNDLE_OUT" | head -n 30 >&2 || true
  FAIL=1
fi
# Nested runtime must also be Developer ID Application (not inferred from TeamIdentifier)
NESTED_OUT=$(codesign -dv --verbose=4 "$APP/Contents/Resources/harpoon/bin/harpoon" 2>&1)
if echo "$NESTED_OUT" | grep -q "Authority=Developer ID Application"; then
  echo "[verify-signatures] PASS: Nested runtime Developer ID Application" >&2
else
  echo "[verify-signatures] FAIL: Nested runtime not Developer ID Application" >&2
  echo "$NESTED_OUT" | head -n 30 >&2 || true
  FAIL=1
fi
# Outer executable must be Developer ID Application
OUTER_EXE_OUT=$(codesign -dv --verbose=4 "$APP/Contents/MacOS/harpoon-desktop" 2>&1)
if echo "$OUTER_EXE_OUT" | grep -q "Authority=Developer ID Application"; then
  echo "[verify-signatures] PASS: Outer executable Developer ID Application" >&2
else
  echo "[verify-signatures] FAIL: Outer executable not Developer ID Application" >&2
  echo "$OUTER_EXE_OUT" | head -n 30 >&2 || true
  FAIL=1
fi
# Outer must NOT have virtualization entitlement
if codesign -d --entitlements :- "$APP/Contents/MacOS/harpoon-desktop" 2>&1 | grep -q "com.apple.security.virtualization"; then
  echo "[verify-signatures] FAIL: outer should not have virtualization entitlement" >&2; FAIL=1
else
  echo "[verify-signatures] PASS: outer no virtualization entitlement" >&2
fi
check "Outer app valid on disk" bash -c "codesign --verify --verbose \"$APP\" 2>&1 | grep -q \"valid on disk\""
check "Bundle deep strict valid" bash -c "codesign --verify --deep --strict --verbose=2 \"$APP\" 2>&1 | grep -q \"valid on disk\""
# Notarization check (optional, after stapling)
if xcrun stapler validate "$APP" 2>&1 | grep -q "validate action worked"; then
  echo "[verify-signatures] PASS: notarization stapled" >&2
else
  echo "[verify-signatures] INFO: notarization pending (run xcrun notarytool submit)" >&2
fi
if [ $FAIL -ne 0 ]; then echo "[verify-signatures] FAIL" >&2; exit 1; fi
echo "[verify-signatures] PASS: release signatures verified" >&2
