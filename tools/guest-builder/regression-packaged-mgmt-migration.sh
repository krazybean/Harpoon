#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SOURCE_BIN="$REPO_ROOT/harpoon/build/harpoon"
SOURCE_KERNEL="$REPO_ROOT/assets/guest/Image-virt"
SOURCE_INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
SOURCE_ROOT="$REPO_ROOT/assets/guest/harpoon-root.img"
SOURCE_INITRAMFS_SHA=$(shasum -a 256 "$SOURCE_INITRAMFS" | cut -d' ' -f1)
DEBUGFS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs
DUMPE2FS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/dumpe2fs
E2FSCK=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/e2fsck
TMP=$(mktemp -d /tmp/harpoon-packaged-migration.XXXXXX)
BUNDLE="$TMP/Harpoon.app/Contents/Resources/harpoon"
BIN="$BUNDLE/bin/harpoon"
LIB="$BUNDLE/lib/harpoon"
KERNEL="$LIB/Image-virt"
INITRAMFS="$LIB/harpoon-initramfs.cpio.gz"
ROOT="$LIB/harpoon-root.img"
DISK="$TMP/old-root.img"
PAYLOAD="$TMP/initramfs"
CANARY="$TMP/docker-canary"
MODULE_DIR=/lib/modules/6.12.94-0-virt
NAT_MODULES=(
  "$MODULE_DIR/kernel/net/netfilter/xt_nat.ko"
  "$MODULE_DIR/kernel/net/netfilter/xt_REDIRECT.ko"
  "$MODULE_DIR/kernel/net/netfilter/xt_MASQUERADE.ko"
)
MISSING_MODULES=("${NAT_MODULES[0]}" "${NAT_MODULES[1]}")
completed=0

fail() { echo "packaged-migration FAIL: $*" >&2; exit 1; }
check() { "$@" || fail "$*"; }
stat_path() { "$DEBUGFS" -R "stat $1" "$DISK" 2>/dev/null; }
fixture() { env -u HARPOON_INITRAMFS -u HARPOON_KERNEL HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$DISK" "$BIN" "$@"; }
stop_fixture() {
  [ -f "$TMP/runtime/runtime.pid" ] || return 0
  fixture stop >/dev/null 2>&1 || true
  [ -f "$TMP/runtime/runtime.pid" ] && kill "$(cat "$TMP/runtime/runtime.pid")" 2>/dev/null || true
}
cleanup() {
  local rc=$?
  stop_fixture
  rm -rf "$TMP"
  if [ "$rc" -eq 0 ] && [ "$completed" -ne 1 ]; then
    echo "packaged-migration FAIL: exited before all acceptance assertions" >&2
    exit 1
  fi
  exit "$rc"
}
trap cleanup EXIT

module_matches_manifest() {
  local path="$1" rel="${1#/}" dump="$TMP/payload-$(basename "$path")" expected_hash expected_mode
  expected_hash=$(awk -v path="$rel" '$1=="F" && $4==path {print $2}' "$PAYLOAD/usr/local/share/harpoon-runtime/modules.manifest")
  expected_mode=$(awk -v path="$rel" '$1=="F" && $4==path {print $3}' "$PAYLOAD/usr/local/share/harpoon-runtime/modules.manifest")
  [ -n "$expected_hash" ] && [ -n "$expected_mode" ] || return 1
  "$DEBUGFS" -R "dump $path $dump" "$DISK" >/dev/null || return 1
  [ "$(shasum -a 256 "$dump" | cut -d' ' -f1)" = "$expected_hash" ] || return 1
  stat_path "$path" | grep -Eq "Mode:  0?$expected_mode"
}
disk_matches_payload() {
  local path="$1" dump="$TMP/payload-$(basename "$path")"
  "$DEBUGFS" -R "dump $path $dump" "$DISK" >/dev/null && cmp -s "$PAYLOAD$path" "$dump"
}
dnat_probe() {
  fixture exec -- sh -c '
    iptables -w 5 -t nat -N HARPOON_DNAT_TEST 2>/dev/null || iptables -w 5 -t nat -F HARPOON_DNAT_TEST
    iptables -w 5 -t nat -A HARPOON_DNAT_TEST -p tcp -d 0/0 --dport 6499 -j DNAT --to-destination 172.18.0.3:6379
    rc=$?
    iptables -w 5 -t nat -F HARPOON_DNAT_TEST 2>/dev/null || true
    iptables -w 5 -t nat -X HARPOON_DNAT_TEST 2>/dev/null || true
    exit "$rc"'
}

check test -x "$SOURCE_BIN"
for path in "$SOURCE_KERNEL" "$SOURCE_INITRAMFS" "$SOURCE_ROOT"; do check test -f "$path"; done
check test -x "$DEBUGFS"
check test -x "$DUMPE2FS"
check test -x "$E2FSCK"
mkdir -p "$BUNDLE/bin" "$LIB" "$PAYLOAD"
cp -p "$SOURCE_BIN" "$BIN"
cp -p "$SOURCE_KERNEL" "$KERNEL"
cp -p "$SOURCE_INITRAMFS" "$INITRAMFS"
(cp -c "$SOURCE_ROOT" "$ROOT" 2>/dev/null || cp -p "$SOURCE_ROOT" "$ROOT") || fail "cannot build fake bundled root"
gzip -dc "$INITRAMFS" | (cd "$PAYLOAD" && cpio -idm >/dev/null 2>&1) || fail "cannot unpack bundled initramfs"
[ "$(shasum -a 256 "$INITRAMFS" | cut -d' ' -f1)" = "$SOURCE_INITRAMFS_SHA" ] || fail "bundled initramfs SHA"
for path in "${NAT_MODULES[@]}" "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias"; do check test -f "$PAYLOAD$path"; done

(cp -c "$ROOT" "$DISK" 2>/dev/null || cp "$ROOT" "$DISK") || fail "cannot clone old root"
set +e
"$E2FSCK" -fy "$DISK" >/dev/null
fsck_status=$?
set -e
[ "$fsck_status" -le 1 ] || fail "template e2fsck status $fsck_status"
printf 'harpoon-packaged-migration-docker-canary\n' > "$CANARY"
for path in "${MISSING_MODULES[@]}"; do "$DEBUGFS" -w -R "rm $path" "$DISK" >/dev/null 2>&1 || true; done
grep -vE 'xt_(nat|REDIRECT)\.ko' "$PAYLOAD$MODULE_DIR/modules.dep" > "$TMP/modules.dep.old"
grep -vE 'xt_(nat|REDIRECT)' "$PAYLOAD$MODULE_DIR/modules.alias" > "$TMP/modules.alias.old"
for metadata in modules.dep modules.alias; do
  "$DEBUGFS" -w -R "rm $MODULE_DIR/$metadata" "$DISK" >/dev/null 2>&1 || true
  "$DEBUGFS" -w -R "write $TMP/$metadata.old $MODULE_DIR/$metadata" "$DISK" >/dev/null || fail "cannot write old $metadata"
done
"$DEBUGFS" -w -R "write $CANARY /var/lib/docker/harpoon-mgmt-canary" "$DISK" >/dev/null || fail "cannot write Docker canary"
set +e
"$E2FSCK" -fy "$DISK" >/dev/null
fsck_status=$?
set -e
[ "$fsck_status" -le 1 ] || fail "fixture e2fsck status $fsck_status"
for path in "${MISSING_MODULES[@]}"; do if stat_path "$path" | grep -q 'Inode:'; then fail "old root still has $path"; fi; done
stat_path "${NAT_MODULES[2]}" | grep -q 'Inode:' || fail "old root lost legacy xt_MASQUERADE"
cmp -s "$PAYLOAD$MODULE_DIR/modules.dep" "$TMP/modules.dep.old" && fail "old metadata still current"
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-before" "$DISK" >/dev/null || fail "fixture Docker canary missing before boot"
cmp -s "$CANARY" "$TMP/canary-before" || fail "fixture Docker canary differs before boot"
before_identity=$(stat -f '%d:%i:%z' "$DISK")
before_uuid=$("$DUMPE2FS" -h "$DISK" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')
check test -n "$before_uuid"

check test "$BIN" -ef "$BUNDLE/bin/harpoon"
echo "packaged-migration: first boot"
fixture start || fail "first boot did not start"
grep -q 'HARPOON_ASSETS_SELECTED kernelOrigin=bundle initramfsOrigin=bundle' "$TMP/runtime/harpoon.log" || fail "assets did not resolve bundle-relative"
fixture exec -- true || fail "management exec true"
for module in xt_nat xt_REDIRECT xt_MASQUERADE; do fixture exec -- modprobe "$module" || fail "management modprobe $module"; done
dnat_probe || fail "DNAT probe"
for _ in $(seq 1 30); do docker --context harpoon version >/dev/null 2>&1 && break; sleep 1; done
docker --context harpoon version >/dev/null 2>&1 || fail "Docker is not ready"
stop_fixture

echo "packaged-migration: checking first boot reconciliation"
after_identity=$(stat -f '%d:%i:%z' "$DISK")
after_uuid=$("$DUMPE2FS" -h "$DISK" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')
[ "$before_identity" = "$after_identity" ] || fail "disk image was replaced"
[ "$before_uuid" = "$after_uuid" ] || fail "filesystem UUID changed"
for path in "${NAT_MODULES[@]}"; do module_matches_manifest "$path" || fail "module manifest mismatch: $path"; done
for path in "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias"; do disk_matches_payload "$path" || fail "module metadata mismatch: $path"; done
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-after" "$DISK" >/dev/null
cmp -s "$CANARY" "$TMP/canary-after" || fail "Docker canary changed"

echo "packaged-migration: checking second boot idempotence"
fixture start || fail "second boot did not start"
fixture exec -- true || fail "second boot management exec true"
for module in xt_nat xt_REDIRECT xt_MASQUERADE; do fixture exec -- modprobe "$module" || fail "second boot modprobe $module"; done
dnat_probe || fail "second boot DNAT probe"
docker --context harpoon version >/dev/null 2>&1 || fail "second boot Docker is not ready"
stop_fixture
for path in "${NAT_MODULES[@]}"; do module_matches_manifest "$path" || fail "second boot module mismatch: $path"; done
for path in "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias"; do disk_matches_payload "$path" || fail "second boot metadata mismatch: $path"; done
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-second" "$DISK" >/dev/null
cmp -s "$CANARY" "$TMP/canary-second" || fail "second boot Docker canary changed"
completed=1
echo "packaged-migration PASS: bundle-relative old root reconciled in place; NAT, Docker, and management preserved"
