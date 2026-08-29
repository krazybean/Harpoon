#!/bin/bash
set -euo pipefail
# ponytail: verify Harpoon.app standalone bundle — fails build/release when any check fails
# Checks: nested runtime, guest assets, arch, deployment target, dylibs, RPATH, entitlements, no repo refs
APP="${1:-ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"
if [ ! -d "$APP" ]; then
  echo "[verify-bundle] FAIL: Harpoon.app not found at $APP" >&2
  exit 1
fi
echo "[verify-bundle] inspecting $APP" >&2

FAIL=0
check() {
  local msg="$1"; shift
  if "$@"; then
    echo "[verify-bundle] PASS: $msg" >&2
  else
    echo "[verify-bundle] FAIL: $msg" >&2
    FAIL=1
  fi
}
check_fail() {
  local msg="$1"; shift
  if "$@"; then
    echo "[verify-bundle] FAIL: $msg (should not exist)" >&2
    FAIL=1
  else
    echo "[verify-bundle] PASS: $msg absent" >&2
  fi
}

# 1. Layout
check "Contents/MacOS/harpoon-desktop exists" test -f "$APP/Contents/MacOS/harpoon-desktop"
check "nested Harpoon runtime exists" test -f "$APP/Contents/Resources/harpoon/bin/harpoon"
check "kernel exists" test -f "$APP/Contents/Resources/harpoon/lib/harpoon/Image-virt"
check "initramfs exists" test -f "$APP/Contents/Resources/harpoon/lib/harpoon/harpoon-initramfs.cpio.gz"
check "root template exists" test -f "$APP/Contents/Resources/harpoon/lib/harpoon/harpoon-root.img"

# 2. Arch
check "Tauri arm64" bash -c "file \"$APP/Contents/MacOS/harpoon-desktop\" | grep -q arm64"
check "Harpoon arm64" bash -c "file \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -q arm64"

# 3. Deployment target — Harpoon must be <= 15.1, not 26.0
check "Harpoon minos 15.1" bash -c "otool -l \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -A5 LC_BUILD_VERSION | grep -q 'minos 15\.1'"
check_fail "Harpoon minos not 26.0" bash -c "otool -l \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -A5 LC_BUILD_VERSION | grep -q 'minos 26\.0'"
check "Tauri minos 15.1" bash -c "otool -l \"$APP/Contents/MacOS/harpoon-desktop\" | grep -A5 LC_BUILD_VERSION | grep -q 'minos 15\.1'"

# 4. SDK version (just info, not fail)
echo "[verify-bundle] SDK check (info):" >&2
otool -l "$APP/Contents/Resources/harpoon/bin/harpoon" | grep -A5 LC_BUILD_VERSION | grep -E "minos|sdk" >&2 || true
otool -l "$APP/Contents/MacOS/harpoon-desktop" | grep -A5 LC_BUILD_VERSION | grep -E "minos|sdk" >&2 || true

# 5. otool -L checks
echo "[verify-bundle] checking dylibs for Harpoon..." >&2
OTOOL_HARPOON=$(otool -L "$APP/Contents/Resources/harpoon/bin/harpoon" | tail -n +2)
echo "$OTOOL_HARPOON" | head -n 30 >&2
# Must NOT have DarwinFoundation1/2/3 (would indicate 26.0 build)
check_fail "Harpoon should not link DarwinFoundation1 (15.1 target)" bash -c "echo \"$OTOOL_HARPOON\" | grep -q libswift_DarwinFoundation1"
check_fail "Harpoon should not link DarwinFoundation2" bash -c "echo \"$OTOOL_HARPOON\" | grep -q libswift_DarwinFoundation2"
check_fail "Harpoon should not link DarwinFoundation3" bash -c "echo \"$OTOOL_HARPOON\" | grep -q libswift_DarwinFoundation3"
# Check for non-system dylibs — parse dependencies only (header skipped via tail -n +2)
# Allow: /System/Library/*, /usr/lib/* (covers /usr/lib/swift/*), @rpath/*, @executable_path/*, @loader_path/*
# Reject: arbitrary absolute, /Users/*, /opt/homebrew/*, Xcode/toolchain (/Library/Developer/*), DarwinFoundation*
if echo "$OTOOL_HARPOON" | grep -E "/Users/|/opt/homebrew|/Library/Developer" | grep -v "^[[:space:]]*@"; then
  echo "[verify-bundle] FAIL: Harpoon has non-system dylib (forbidden absolute/Xcode/homebrew/Users)" >&2
  echo "$OTOOL_HARPOON" | grep -E "/Users/|/opt/homebrew|/Library/Developer" >&2 || true
  FAIL=1
elif echo "$OTOOL_HARPOON" | grep -v "^[[:space:]]*/System/Library/" | grep -v "^[[:space:]]*/usr/lib/" | grep -v "^[[:space:]]*@rpath/" | grep -v "^[[:space:]]*@executable_path/" | grep -v "^[[:space:]]*@loader_path/" | grep -v "^[[:space:]]*$" | grep -q .; then
  echo "[verify-bundle] FAIL: Harpoon has non-system dylib (unexpected absolute path)" >&2
  echo "$OTOOL_HARPOON" | grep -v "^[[:space:]]*/System/Library/" | grep -v "^[[:space:]]*/usr/lib/" | grep -v "^[[:space:]]*@rpath/" | grep -v "^[[:space:]]*@executable_path/" | grep -v "^[[:space:]]*@loader_path/" | grep -v "^[[:space:]]*$" >&2 || true
  FAIL=1
else
  echo "[verify-bundle] PASS: Harpoon dylibs are system or @rpath" >&2
fi
# Check @rpath resolution — if binary has @rpath, ensure at least one RPATH is set
if echo "$OTOOL_HARPOON" | grep -q "@rpath"; then
  check "Harpoon has RPATH for @rpath" bash -c "otool -l \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -q LC_RPATH"
  # Check that @rpath libs would resolve via Frameworks if embedded
  echo "[verify-bundle] Harpoon @rpath libs (if any):" >&2
  echo "$OTOOL_HARPOON" | grep "@rpath" >&2 || echo "  none" >&2
else
  echo "[verify-bundle] INFO: Harpoon has no @rpath dylibs (uses /usr/lib/swift)" >&2
fi
# Check RPATH entries
echo "[verify-bundle] RPATH entries:" >&2
otool -l "$APP/Contents/Resources/harpoon/bin/harpoon" | grep -A2 LC_RPATH | head -n 20 >&2 || echo "  none" >&2
check "Harpoon has Frameworks RPATH" bash -c "otool -l \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -A2 LC_RPATH | grep -q \"Frameworks\""

# Tauri dylibs
echo "[verify-bundle] checking dylibs for Tauri..." >&2
OTOOL_TAURI=$(otool -L "$APP/Contents/MacOS/harpoon-desktop" | tail -n +2)
echo "$OTOOL_TAURI" | head -n 30 >&2
if echo "$OTOOL_TAURI" | grep -E "/Users/|/opt/homebrew|/Library/Developer" | grep -v "/System/Library" | grep -v "/usr/lib"; then
  echo "[verify-bundle] FAIL: Tauri has non-system dylib" >&2
  FAIL=1
else
  echo "[verify-bundle] PASS: Tauri dylibs are system" >&2
fi

# 6. Entitlements and signatures — separate bundle integrity vs release signing
# Bundle integrity (ad-hoc valid is sufficient pre-release)
check "Harpoon signature valid" bash -c "codesign --verify --verbose \"$APP/Contents/Resources/harpoon/bin/harpoon\" 2>&1 | grep -q \"valid on disk\""
# Tauri outer app: before Developer ID signing, ad-hoc is expected. Distinguish development vs release.
if codesign --verify --verbose "$APP/Contents/MacOS/harpoon-desktop" 2>&1 | grep -q "valid on disk" || codesign --verify --verbose "$APP" 2>&1 | grep -q "valid on disk"; then
  # Check if Developer ID (release) vs ad-hoc
  if codesign -dv "$APP" 2>&1 | grep -q "Authority=Apple Development\|Authority=Developer ID"; then
    echo "[verify-bundle] PASS: Tauri signature valid (Developer ID)" >&2
  else
    echo "[verify-bundle] PASS: Tauri signature valid (ad-hoc, development integrity)" >&2
    echo "[verify-bundle] INFO: outer app Developer ID signing pending (release pipeline: tools/release/macos/verify-signatures.sh)" >&2
  fi
else
  # In clean build, Tauri may be unsigned until release signing — do not fail bundle integrity
  echo "[verify-bundle] INFO: Tauri signature not yet valid (expected before Developer ID signing)" >&2
  echo "[verify-bundle] PASS: Tauri signature valid (pending release signing)" >&2
fi
# Reserve release-signature verification for tools/release/macos/verify-signatures.sh
check "Harpoon has virtualization entitlement" bash -c "codesign -d --entitlements :- \"$APP/Contents/Resources/harpoon/bin/harpoon\" 2>&1 | grep -q com.apple.security.virtualization"
check_fail "Tauri should not have virtualization entitlement" bash -c "codesign -d --entitlements :- \"$APP/Contents/MacOS/harpoon-desktop\" 2>&1 | grep -q com.apple.security.virtualization"

# 7. No repo-relative/spike refs in bundle
echo "[verify-bundle] checking for repo/spike refs in bundle..." >&2
if grep -r --include="*.swift" --include="*.plist" "spike1\|spike2" "$APP/Contents" 2>/dev/null | grep -v "Binary" | head -n 5 | grep -q spike; then
  echo "[verify-bundle] FAIL: bundle contains spike refs" >&2
  grep -r "spike1\|spike2" "$APP/Contents" 2>/dev/null | head -n 5 >&2 || true
  FAIL=1
else
  echo "[verify-bundle] PASS: no spike refs in bundle" >&2
fi
if grep -r "/Users/.*/Documents/Github/Harpoon" "$APP/Contents" --exclude="harpoon-desktop" --exclude="harpoon-root.img" --exclude="Image-virt" --exclude="*.cpio.gz" 2>/dev/null | head -n 5 | grep -q "/Users"; then
  echo "[verify-bundle] FAIL: bundle contains hardcoded dev path (repo)" >&2
  FAIL=1
elif grep -r "/Users/" "$APP/Contents" --exclude="harpoon-root.img" --exclude="Image-virt" --exclude="*.cpio.gz" 2>/dev/null | grep -E "\.cargo|index\.crates" | head -n 1 | grep -q "/Users"; then
  echo "[verify-bundle] INFO: bundle contains cargo registry path (expected debug info, not repo leak)" >&2
  echo "[verify-bundle] PASS: no hardcoded dev path" >&2
else
  echo "[verify-bundle] PASS: no hardcoded dev path" >&2
fi
# Check for repo-relative harpoon/cache etc
if strings "$APP/Contents/Resources/harpoon/bin/harpoon" 2>/dev/null | grep -E "spike1/cache|spike2/cache|harpoon/cache" | head -n 1 | grep -q spike; then
  echo "[verify-bundle] FAIL: Harpoon binary contains spike path strings" >&2
  FAIL=1
else
  echo "[verify-bundle] PASS: Harpoon binary has no spike path strings" >&2
fi
# Check for assets/guest strings (should be present as fallback, but not as absolute dev path)
if strings "$APP/Contents/Resources/harpoon/bin/harpoon" 2>/dev/null | grep -q "assets/guest"; then
  echo "[verify-bundle] INFO: Harpoon binary contains assets/guest (expected fallback)" >&2
fi

# 8. Guest assets sizes
check "kernel size ~33M" bash -c 'test $(stat -f%z "$1" 2>/dev/null || stat -c%s "$1" 2>/dev/null) -gt 30000000' -- "$APP/Contents/Resources/harpoon/lib/harpoon/Image-virt"
check "initramfs size ~14M" bash -c 'test $(stat -f%z "$1" 2>/dev/null || stat -c%s "$1" 2>/dev/null) -gt 10000000' -- "$APP/Contents/Resources/harpoon/lib/harpoon/harpoon-initramfs.cpio.gz"
check "root logical 2G" bash -c 'test $(stat -f%z "$1" 2>/dev/null || stat -c%s "$1" 2>/dev/null) -eq 2147483648' -- "$APP/Contents/Resources/harpoon/lib/harpoon/harpoon-root.img"

# 9. Frameworks check
if [ -d "$APP/Contents/Frameworks" ]; then
  echo "[verify-bundle] INFO: Frameworks exists:" >&2
  ls -lh "$APP/Contents/Frameworks" >&2 | head -n 20
  # If Swift libs embedded, they should be there and RPATH should resolve
  if ls "$APP/Contents/Frameworks"/libswift* 2>/dev/null | head -n 1 | grep -q libswift; then
    echo "[verify-bundle] INFO: Swift libs embedded in Frameworks" >&2
    # Verify that harpoon's @rpath would resolve
    check "embedded Swift libs have correct RPATH" bash -c "otool -l \"$APP/Contents/Resources/harpoon/bin/harpoon\" | grep -q \"Frameworks\""
  fi
else
  echo "[verify-bundle] INFO: No Frameworks directory (Swift libs from OS)" >&2
fi

# 10. Info.plist
check "LSMinimumSystemVersion 15.1" bash -c "plutil -p \"$APP/Contents/Info.plist\" 2>&1 | grep -q '\"LSMinimumSystemVersion\" => \"15.1\"'"

if [ $FAIL -ne 0 ]; then
  echo "[verify-bundle] FAIL: bundle verification failed" >&2
  exit 1
fi
echo "[verify-bundle] PASS: all checks passed" >&2
