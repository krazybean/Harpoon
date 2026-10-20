#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/guest-arch.sh"
DEFAULT_ASSET_DIR="$(guest_asset_dir "$REPO_ROOT/assets/guest")"
ASSET_DIR="$DEFAULT_ASSET_DIR"
KERNEL="$ASSET_DIR/Image-virt"
INITRAMFS="$ASSET_DIR/harpoon-initramfs.cpio.gz"
MANIFEST="$ASSET_DIR/guest-assets.manifest"
KERNEL_VERSION="$GUEST_KERNEL_VERSION"

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }

write_manifest() {
  [ -f "$KERNEL" ] || { echo "[guest-assets] missing kernel: $KERNEL" >&2; return 1; }
  [ -f "$INITRAMFS" ] || { echo "[guest-assets] missing initramfs: $INITRAMFS" >&2; return 1; }
  local tmp="$MANIFEST.tmp.$$"
  {
    printf 'harpoon-guest-assets-v1\n'
    printf 'kernel_version %s\n' "$KERNEL_VERSION"
    printf 'kernel_sha256 %s\n' "$(sha256 "$KERNEL")"
    printf 'initramfs_sha256 %s\n' "$(sha256 "$INITRAMFS")"
  } > "$tmp"
  mv "$tmp" "$MANIFEST"
}

verify_manifest() {
  [ -s "$MANIFEST" ] || { echo "GUEST_ASSET_MISMATCH missing manifest: $MANIFEST" >&2; return 1; }
  grep -q '^harpoon-guest-assets-v1$' "$MANIFEST" || { echo "GUEST_ASSET_MISMATCH invalid manifest: $MANIFEST" >&2; return 1; }
  local expected_kernel expected_initramfs
  expected_kernel="$(awk '$1 == "kernel_sha256" { print $2 }' "$MANIFEST")"
  expected_initramfs="$(awk '$1 == "initramfs_sha256" { print $2 }' "$MANIFEST")"
  [ -n "$expected_kernel" ] && [ "$expected_kernel" = "$(sha256 "$KERNEL")" ] || { echo "GUEST_ASSET_MISMATCH kernel SHA does not match manifest" >&2; return 1; }
  [ -n "$expected_initramfs" ] && [ "$expected_initramfs" = "$(sha256 "$INITRAMFS")" ] || { echo "GUEST_ASSET_MISMATCH initramfs SHA does not match manifest" >&2; return 1; }
}

case "${1:-}" in
  --verify) ASSET_DIR="${2:-$DEFAULT_ASSET_DIR}"; KERNEL="$ASSET_DIR/Image-virt"; INITRAMFS="$ASSET_DIR/harpoon-initramfs.cpio.gz"; MANIFEST="$ASSET_DIR/guest-assets.manifest"; verify_manifest ;;
  --write) ASSET_DIR="${2:-$DEFAULT_ASSET_DIR}"; KERNEL="$ASSET_DIR/Image-virt"; INITRAMFS="$ASSET_DIR/harpoon-initramfs.cpio.gz"; MANIFEST="$ASSET_DIR/guest-assets.manifest"; write_manifest ;;
  *) echo "usage: $0 --write [asset-dir] | --verify [asset-dir]" >&2; exit 2 ;;
esac
