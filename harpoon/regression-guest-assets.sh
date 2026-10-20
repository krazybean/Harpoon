#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-guest-assets.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

BIN="$TMP/check"
xcrun swiftc -target "arm64-apple-macosx$(cat "$(dirname "$0")/MIN_MACOS")" \
  "$ROOT/harpoon/Sources/RuntimeConfig.swift" "$ROOT/harpoon/GuestAssetFixtureMain.swift" \
  -framework Virtualization -o "$BIN" -module-cache-path "$TMP/module-cache"

make_fixture() {
  local dir="$1"
  mkdir -p "$dir"
  printf 'kernel fixture\n' > "$dir/Image-virt"
  printf 'initramfs fixture\n' > "$dir/harpoon-initramfs.cpio.gz"
  bash "$ROOT/tools/guest-builder/guest-assets-manifest.sh" --write "$dir"
}

SOURCE="$TMP/source/assets/guest"
BUNDLE="$TMP/bundle/lib/harpoon"
make_fixture "$SOURCE"
make_fixture "$BUNDLE"

"$BIN" "$SOURCE/Image-virt" "$SOURCE/harpoon-initramfs.cpio.gz" | grep -q GUEST_ASSET_MATCH
"$BIN" "$BUNDLE/Image-virt" "$BUNDLE/harpoon-initramfs.cpio.gz" | grep -q GUEST_ASSET_MATCH
printf 'tampered\n' >> "$BUNDLE/harpoon-initramfs.cpio.gz"
if mismatch="$($BIN "$BUNDLE/Image-virt" "$BUNDLE/harpoon-initramfs.cpio.gz" 2>&1 || true)" && printf '%s\n' "$mismatch" | grep -q GUEST_ASSET_MISMATCH; then
  echo "guest-assets mismatch rejects PASS"
else
  echo "guest-assets mismatch rejects FAIL" >&2
  exit 1
fi
echo "guest-assets source pair PASS"
echo "guest-assets bundle pair PASS"
