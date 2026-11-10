#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP=$(mktemp -d /tmp/harpoon-assets.XXXXXX)
BIN="$TMP/harpoon"
APP="$TMP/Harpoon.app/Contents/Resources/harpoon"
trap 'rm -rf "$TMP"' EXIT

pass() { echo "asset-resolution $1 PASS"; }
fail() { echo "asset-resolution $1 FAIL" >&2; exit 1; }
check() { "$@" || fail "$1"; }

# Every runtime source, as harpoon/build.sh compiles them (main.swift calls into all of them), so this list cannot drift.
# Output and module cache stay in $TMP; harpoon/build/harpoon is never touched.
xcrun swiftc -target "arm64-apple-macosx$(cat "$(dirname "$0")/MIN_MACOS")" \
  "$ROOT"/harpoon/Sources/*.swift \
  -framework Virtualization -o "$BIN" -module-cache-path "$TMP/mcache" >/dev/null

mkdir -p "$APP/bin" "$APP/lib/harpoon" "$TMP/decoy/assets/guest" "$TMP/bin"
cp "$BIN" "$APP/bin/harpoon"
touch "$APP/lib/harpoon/Image-virt" "$APP/lib/harpoon/harpoon-initramfs.cpio.gz" "$APP/lib/harpoon/harpoon-root.img"
touch "$TMP/decoy/assets/guest/Image-virt" "$TMP/decoy/assets/guest/harpoon-initramfs.cpio.gz" "$TMP/decoy/assets/guest/harpoon-root.img"

doctor() { (cd "$TMP/decoy" && HARPOON_TEST_TMPDIR="$TMP/state-$1" "$2" doctor); }
# doctor exits 1 on any unrelated failing check (e.g. the Docker context); only the resolved paths are asserted
OUT=$(doctor direct "$APP/bin/harpoon" || true)
printf '%s\n' "$OUT" | grep -q "kernel $APP/lib/harpoon/Image-virt" || fail unrelated-cwd
printf '%s\n' "$OUT" | grep -q "initramfs $APP/lib/harpoon/harpoon-initramfs.cpio.gz" || fail direct-bundle
pass unrelated-cwd
pass direct-bundle

ln -s "$APP/bin/harpoon" "$TMP/bin/harpoon"
OUT=$(doctor symlink "$TMP/bin/harpoon" || true)
printf '%s\n' "$OUT" | grep -q "kernel $APP/lib/harpoon/Image-virt" || fail symlink
pass symlink

OUT=$(cd "$ROOT" && HARPOON_TEST_TMPDIR="$TMP/state-source" "$BIN" doctor || true)
printf '%s\n' "$OUT" | grep -q "kernel $ROOT/assets/guest/Image-virt" || fail source-tree
pass source-tree

touch "$TMP/override-kernel" "$TMP/override-initramfs"
OUT=$(cd "$TMP/decoy" && HARPOON_TEST_TMPDIR="$TMP/state-override" HARPOON_KERNEL="$TMP/override-kernel" HARPOON_INITRAMFS="$TMP/override-initramfs" "$APP/bin/harpoon" doctor || true)
printf '%s\n' "$OUT" | grep -q "kernel $TMP/override-kernel" || fail overrides
printf '%s\n' "$OUT" | grep -q "initramfs $TMP/override-initramfs" || fail overrides
pass overrides

rm "$APP/lib/harpoon/harpoon-initramfs.cpio.gz"
OUT=$(doctor missing "$APP/bin/harpoon" || true)
printf '%s\n' "$OUT" | grep -q "FAIL  initramfs $APP/lib/harpoon/harpoon-initramfs.cpio.gz" || fail missing-bundle
pass missing-bundle
