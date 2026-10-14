#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-root-builder-regression.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/assets/guest"
! HARPOON_ROOT_BUILDER= HARPOON_ROOT_OUT="$TMP/assets/guest/harpoon-root.img" bash "$REPO_ROOT/tools/guest-builder/build-root.sh" 2>"$TMP/error"
grep -q 'requires HARPOON_ROOT_BUILDER=vz' "$TMP/error"

truncate -s 2147483648 "$TMP/assets/guest/harpoon-root.img"
! HARPOON_ROOT_BUILDER=vz HARPOON_ROOT_OUT="$TMP/assets/guest/harpoon-root.img" bash "$REPO_ROOT/tools/guest-builder/build-root.sh" 2>"$TMP/existing-error"
grep -q 'refusing historical/cached root image' "$TMP/existing-error"

cat > "$TMP/assets/guest/harpoon-root.img.manifest" <<'EOF'
alpine-root-builder-v1
alpine-branch v3.22
repositories https://dl-cdn.alpinelinux.org/alpine/v3.22/main https://dl-cdn.alpinelinux.org/alpine/v3.22/community
packages docker-engine docker-cli containerd runc socat python3 ca-certificates e2fsprogs e2fsprogs-extra iproute2 iptables nftables kmod
contract dockerd docker containerd runc socat python3 resize2fs ip iptables nft modprobe
EOF
HARPOON_VERIFY_ROOT_SKIP_SCAN=1 HARPOON_ROOT_BUILDER=vz bash "$REPO_ROOT/tools/guest-builder/verify-root.sh" "$TMP/assets/guest/harpoon-root.img" >/dev/null
echo "root-builder PASS"
