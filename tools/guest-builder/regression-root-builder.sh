#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-root-builder-regression.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/assets/guest"
VZ_SCRIPT="$REPO_ROOT/tools/guest-builder/build-root-vz.sh"
grep -q 'codesign --entitlements "$ENTITLEMENTS" --force -s - "$WORK/AlpineRootBuilder"' "$VZ_SCRIPT"
grep -q 'com.apple.security.virtualization' "$VZ_SCRIPT"
grep -q 'EXPECTED_MODLOOP_SHA' "$VZ_SCRIPT"
grep -q 'virtio_blk virtio_net af_packet' "$REPO_ROOT/tools/guest-builder/src/alpine-root-builder-init"
grep -q '/bin/busybox' "$REPO_ROOT/tools/guest-builder/src/alpine-root-builder-init"
grep -q 'bb sleep 5' "$REPO_ROOT/tools/guest-builder/src/alpine-root-builder-init"
! grep -Eq '(^|[;&])[[:space:]]*sleep[[:space:]]' "$REPO_ROOT/tools/guest-builder/src/alpine-root-builder-init"
grep -q 'Kernel panic - not syncing' "$REPO_ROOT/tools/guest-builder/AlpineRootBuilder.swift"
grep -q 'timeout' "$REPO_ROOT/tools/guest-builder/AlpineRootBuilder.swift"

MOCK_BIN="$TMP/bin"
mkdir -p "$MOCK_BIN" "$TMP/seed"
(cd "$TMP/seed" && printf 'seed\n' > seed && find . -print0 | cpio --null -o -H newc --quiet | gzip -n -9 > "$TMP/initramfs-virt")
cat > "$MOCK_BIN/swiftc" <<'EOF'
#!/bin/bash
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then OUT="$2"; shift 2; else shift; fi
done
printf '#!/bin/sh\nexit 0\n' > "$OUT"
chmod +x "$OUT"
EOF
cat > "$MOCK_BIN/curl" <<'EOF'
#!/bin/bash
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then OUT="$2"; shift 2; else shift; fi
done
cp "$MOCK_INITRAMFS" "$OUT"
EOF
cat > "$MOCK_BIN/shasum" <<'EOF'
#!/bin/bash
case "$3" in
  *modloop*) echo da142869626ec9d4543ab46482af10b5ff2c8c1c2e01068ddf32360316a06218  "$3" ;;
  *) echo 384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c  "$3" ;;
esac
EOF
cat > "$MOCK_BIN/codesign" <<'EOF'
#!/bin/bash
if [ "$1" = "-d" ]; then
  echo 'com.apple.security.virtualization = true'
else
  printf '%s\n' "$*" >> "$CODESIGN_LOG"
fi
EOF
cat > "$MOCK_BIN/unsquashfs" <<'EOF'
#!/bin/bash
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-d" ]; then OUT="$2"; shift 2; else shift; fi
done
mkdir -p "$OUT/modules/6.12.110-0-virt"
printf '%s\n' 'fixture' > "$OUT/modules/6.12.110-0-virt/modules.dep"
EOF
chmod +x "$MOCK_BIN"/*
MOCK_INITRAMFS="$TMP/initramfs-virt" CODESIGN_LOG="$TMP/codesign.log" \
  HARPOON_ROOT_OUT="$TMP/assets/guest/mock-root.img" PATH="$MOCK_BIN:$PATH" \
  bash "$VZ_SCRIPT" >/dev/null
grep -Fq -- "--entitlements $REPO_ROOT/harpoon/entitlements.plist" "$TMP/codesign.log"

! HARPOON_ROOT_OUT="$TMP/assets/guest/harpoon-root.img" bash "$REPO_ROOT/tools/guest-builder/build-root.sh" 2>"$TMP/error"
grep -q 'explicit --builder vz is required' "$TMP/error"

truncate -s 2147483648 "$TMP/assets/guest/harpoon-root.img"
! HARPOON_ROOT_OUT="$TMP/assets/guest/harpoon-root.img" bash "$REPO_ROOT/tools/guest-builder/build-root.sh" --builder vz 2>"$TMP/existing-error"
grep -q 'refusing historical/cached root image' "$TMP/existing-error"

cat > "$TMP/assets/guest/harpoon-root.img.manifest" <<'EOF'
alpine-root-builder-v1
alpine-branch v3.22
repositories https://dl-cdn.alpinelinux.org/alpine/v3.22/main https://dl-cdn.alpinelinux.org/alpine/v3.22/community
packages docker-engine docker-cli containerd runc socat python3 ca-certificates e2fsprogs e2fsprogs-extra iproute2 iptables nftables kmod
contract dockerd docker containerd runc socat python3 resize2fs ip iptables nft modprobe
EOF
HARPOON_VERIFY_ROOT_SKIP_SCAN=1 bash "$REPO_ROOT/tools/guest-builder/verify-root.sh" "$TMP/assets/guest/harpoon-root.img" >/dev/null
echo "root-builder PASS"
