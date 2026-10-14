#!/bin/bash
set -euo pipefail
# ponytail: one disposable VZ builder; real apk runs in Alpine, not macOS or Docker.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="${HARPOON_ROOT_OUT:-$REPO_ROOT/assets/guest/harpoon-root.img}"
CACHE="$REPO_ROOT/assets/guest/.cache"
WORK="$(mktemp -d /tmp/harpoon-root-builder.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

EXPECTED_INITRAMFS_SHA="384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c"
INITRAMFS_CACHE="$CACHE/initramfs-virt"
BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot"
mkdir -p "$CACHE" "$(dirname "$OUT")"
for tool in curl gzip cpio swiftc codesign shasum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "[build-root-vz] FAIL: missing host tool $tool" >&2; exit 1; }
done
ENTITLEMENTS="$REPO_ROOT/harpoon/entitlements.plist"
[ -f "$ENTITLEMENTS" ] || { echo "[build-root-vz] FAIL: missing canonical entitlements $ENTITLEMENTS" >&2; exit 1; }

fetch_pinned() {
  local tmp="$1.tmp" actual
  if [ -f "$1" ] && [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$2" ]; then return; fi
  curl -L --fail -o "$tmp" "$BASE/initramfs-virt"
  actual="$(shasum -a 256 "$tmp" | cut -d' ' -f1)"
  [ "$actual" = "$2" ] || { rm -f "$tmp"; echo "[build-root-vz] initramfs SHA mismatch" >&2; exit 1; }
  mv "$tmp" "$1"
}

[ -f "$REPO_ROOT/assets/guest/Image-virt" ] || { echo "[build-root-vz] missing Image-virt; run fetch-kernel.sh" >&2; exit 1; }
fetch_pinned "$INITRAMFS_CACHE" "$EXPECTED_INITRAMFS_SHA"

STAGE="$WORK/initramfs"
mkdir -p "$STAGE"
(cd "$STAGE" && gzip -dc "$INITRAMFS_CACHE" | cpio -idmu --quiet)
mkdir -p "$STAGE/builder-payload"
cp "$SCRIPT_DIR/src/alpine-root-builder-init" "$STAGE/init"
cp "$SCRIPT_DIR/src/init" "$STAGE/builder-payload/init"
cp "$SCRIPT_DIR/src/harpoon-mgmt" "$STAGE/builder-payload/harpoon-mgmt"
cp "$SCRIPT_DIR/src/harpoon-mgmt-wrapper" "$STAGE/builder-payload/harpoon-mgmt-wrapper"
chmod 0755 "$STAGE/init" "$STAGE/builder-payload"/*
find "$STAGE" -exec touch -h -d '2025-01-01 00:00:00' {} +
(cd "$STAGE" && find . -print0 | LC_ALL=C sort -z | cpio --null -o -H newc --quiet | gzip -n -9 > "$WORK/builder-initramfs.cpio.gz")

if command -v truncate >/dev/null 2>&1; then
  truncate -s 2147483648 "$WORK/harpoon-root.img"
elif command -v mkfile >/dev/null 2>&1; then
  mkfile -n 2g "$WORK/harpoon-root.img"
else
  echo "[build-root-vz] FAIL: need native truncate or mkfile to create the blank target" >&2
  exit 1
fi
SWIFT_CACHE="$WORK/swift-module-cache"
mkdir -p "$SWIFT_CACHE"
CLANG_MODULE_CACHE_PATH="$SWIFT_CACHE" swiftc "$SCRIPT_DIR/AlpineRootBuilder.swift" -o "$WORK/AlpineRootBuilder"
echo "[build-root-vz] signing AlpineRootBuilder with $ENTITLEMENTS" >&2
codesign --entitlements "$ENTITLEMENTS" --force -s - "$WORK/AlpineRootBuilder" 2>&1 || {
  echo "[build-root-vz] FAIL: codesign failed for AlpineRootBuilder" >&2
  exit 1
}
ENTITLEMENT_DUMP="$WORK/AlpineRootBuilder.entitlements"
if ! codesign -d --entitlements - "$WORK/AlpineRootBuilder" >"$ENTITLEMENT_DUMP" 2>&1; then
  echo "[build-root-vz] FAIL: unable to inspect AlpineRootBuilder entitlements" >&2
  cat "$ENTITLEMENT_DUMP" >&2
  exit 1
fi
if ! grep -q 'com.apple.security.virtualization' "$ENTITLEMENT_DUMP"; then
  echo "[build-root-vz] FAIL: AlpineRootBuilder is missing com.apple.security.virtualization" >&2
  cat "$ENTITLEMENT_DUMP" >&2
  exit 1
fi
"$WORK/AlpineRootBuilder" "$REPO_ROOT/assets/guest/Image-virt" "$WORK/builder-initramfs.cpio.gz" "$WORK/harpoon-root.img" "$WORK/serial.log"
mv "$WORK/harpoon-root.img" "$OUT"
cat > "$OUT.manifest" <<EOF
alpine-root-builder-v1
alpine-branch v3.22
repositories https://dl-cdn.alpinelinux.org/alpine/v3.22/main https://dl-cdn.alpinelinux.org/alpine/v3.22/community
packages docker-engine docker-cli containerd runc socat python3 ca-certificates e2fsprogs e2fsprogs-extra iproute2 iptables nftables kmod
contract dockerd docker containerd runc socat python3 resize2fs ip iptables nft modprobe
EOF
printf 'kernel %s\n' "$(shasum -a 256 "$REPO_ROOT/assets/guest/Image-virt" | cut -d' ' -f1)" >> "$OUT.manifest"
printf 'builder-initramfs %s\n' "$(shasum -a 256 "$INITRAMFS_CACHE" | cut -d' ' -f1)" >> "$OUT.manifest"
printf 'root %s\n' "$(shasum -a 256 "$OUT" | cut -d' ' -f1)" >> "$OUT.manifest"
echo "[build-root-vz] populated Alpine root: $OUT" >&2
