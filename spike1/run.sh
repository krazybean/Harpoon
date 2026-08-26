#!/bin/bash
set -euo pipefail
# ponytail: minimal runner, no hidden state, repeatable by another dev
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CACHE="$SCRIPT_DIR/cache"
BUILD="$SCRIPT_DIR/build"
mkdir -p "$BUILD"
KERNEL="$CACHE/Image-virt"
# Fallback to vmlinuz if Image not present (but spike requires uncompressed)
if [ ! -f "$KERNEL" ]; then KERNEL="$CACHE/vmlinuz-virt"; echo "[spike1] WARNING: Image-virt not found, falling back to vmlinuz-virt (will fail on Apple Silicon)">&2; fi
INITRAMFS_MARKER="$CACHE/harpoon-initramfs.cpio.gz"
INITRAMFS_ALPINE="$CACHE/initramfs-virt"
# choose initramfs: prefer harpoon minimal if present else alpine
if [ -f "$INITRAMFS_MARKER" ]; then
  INITRAMFS="$INITRAMFS_MARKER"
else
  INITRAMFS="$INITRAMFS_ALPINE"
fi
# Build Swift binary
BIN="$BUILD/harpoon-spike1"
echo "[spike1] building Swift VM runner..." >&2
swiftc "$SCRIPT_DIR/swift/main.swift" -framework Virtualization -o "$BIN" -module-cache-path /tmp/harpoon-spike-mcache 2>&1
echo "[spike1] signing with entitlement..." >&2
codesign --entitlements "$SCRIPT_DIR/entitlements.plist" --force -s - "$BIN" 2>&1
echo "[spike1] checking entitlement..." >&2
codesign -d --entitlements - "$BIN" 2>&1 | grep -q "com.apple.security.virtualization" && echo "[spike1] entitlement OK" >&2 || { echo "[spike1] entitlement missing!" >&2; exit 1; }
echo "[spike1] isSupported pre-check..." >&2
# quick validate via helper already tested
echo "[spike1] invoking VM boot kernel=$(basename "$KERNEL") initramfs=$(basename "$INITRAMFS") timeout=40" >&2
sw_vers >&2
uname -m >&2
swiftc --version >&2
echo "[spike1] cache listing:" >&2
ls -lh "$CACHE" >&2
# Ensure files exist
[ -f "$KERNEL" ] || { echo "kernel missing, run spike1/fetch_guest.sh first" >&2; exit 1; }
[ -f "$INITRAMFS" ] || { echo "initramfs missing" >&2; exit 1; }
# run
echo "[spike1] ---- VM START ----" >&2
set +e
"$BIN" "$KERNEL" "$INITRAMFS" 40 2> "$BUILD/vm.log" | tee "$BUILD/guest.out"
EXIT=$?
set -e
echo "[spike1] ---- VM EXIT code=$EXIT ----" >&2
cat "$BUILD/vm.log" >&2
echo "[spike1] guest output tail:" >&2
tail -c 2000 "$BUILD/guest.out" >&2 || true
# also check for boot marker in combined logs
if grep -q "BOOT_DETECTED\|HARPOON_SPIKE_OK\|Linux version" "$BUILD/vm.log" "$BUILD/guest.out" 2>/dev/null; then
  echo "[spike1] BOOT PROVED" >&2
else
  echo "[spike1] BOOT NOT DETECTED" >&2
fi
if grep -q "SHUTDOWN_OK" "$BUILD/vm.log" 2>/dev/null; then
  echo "[spike1] SHUTDOWN PROVED" >&2
else
  echo "[spike1] SHUTDOWN NOT PROVED (check log)" >&2
fi
echo "[spike1] logs: $BUILD/vm.log + $BUILD/guest.out" >&2
exit $EXIT
