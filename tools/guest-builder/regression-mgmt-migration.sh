#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$REPO_ROOT/harpoon/build/harpoon"
ROOT="$REPO_ROOT/assets/guest/harpoon-root.img"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
DEBUGFS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs
DUMPE2FS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/dumpe2fs
E2FSCK=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/e2fsck
TMP=$(mktemp -d /tmp/harpoon-mgmt-migration.XXXXXX)
DISK="$TMP/old-root.img"
CANARY="$TMP/docker-canary"
PAYLOAD="$TMP/initramfs"
MODULE_DIR=/lib/modules/6.12.94-0-virt
NAT_MODULES=(
  "$MODULE_DIR/kernel/net/netfilter/xt_nat.ko"
  "$MODULE_DIR/kernel/net/netfilter/xt_REDIRECT.ko"
  "$MODULE_DIR/kernel/net/netfilter/xt_MASQUERADE.ko"
)
completed=0
stop_fixture() {
  [ -f "$TMP/runtime/runtime.pid" ] || return 0
  HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" stop >/dev/null 2>&1 || true
  [ -f "$TMP/runtime/runtime.pid" ] && kill "$(cat "$TMP/runtime/runtime.pid")" 2>/dev/null || true
}
cleanup() {
  local rc=$?
  stop_fixture
  rm -rf "$TMP"
  if [ "$rc" -eq 0 ] && [ "$completed" -ne 1 ]; then
    echo "mgmt-migration FAIL: exited before all acceptance assertions" >&2
    exit 1
  fi
  exit "$rc"
}
trap cleanup EXIT

fail() { echo "mgmt-migration FAIL: $*" >&2; exit 1; }
check() { "$@" || fail "$*"; }
stat_path() { "$DEBUGFS" -R "stat $1" "$DISK" 2>/dev/null; }
mode_owner() { stat_path "$1" | grep -Eq 'Mode:  0755' && stat_path "$1" | grep -q 'User:     0   Group:     0'; }
mode_exec() { stat_path "$1" | grep -Eq 'Mode:  0755'; }
disk_matches_payload() { local path="$1" dump="$TMP/payload-$(basename "$path")"; "$DEBUGFS" -R "dump $path $dump" "$DISK" >/dev/null && cmp -s "$PAYLOAD$path" "$dump"; }

check test -x "$BIN"
check test -f "$ROOT"
check test -f "$INITRAMFS"
check test -x "$DEBUGFS"
check test -x "$DUMPE2FS"
check test -x "$E2FSCK"
mkdir -p "$PAYLOAD"
gzip -dc "$INITRAMFS" | (cd "$PAYLOAD" && cpio -idm >/dev/null 2>&1) || fail "cannot unpack initramfs payload"
for path in "${NAT_MODULES[@]}" "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias"; do check test -f "$PAYLOAD$path"; done
(cp -c "$ROOT" "$DISK" 2>/dev/null || cp "$ROOT" "$DISK") || fail "cannot clone old root"
set +e
"$E2FSCK" -fy "$DISK" >/dev/null
fsck_status=$?
set -e
[ "$fsck_status" -le 1 ] || fail "template e2fsck status $fsck_status"
printf 'harpoon-migration-docker-canary\n' > "$CANARY"
"$DEBUGFS" -w -R 'rm /usr/local/bin/harpoon-mgmt' "$DISK" >/dev/null 2>&1 || true
for path in "${NAT_MODULES[@]}"; do "$DEBUGFS" -w -R "rm $path" "$DISK" >/dev/null 2>&1 || true; done
"$DEBUGFS" -w -R "write $CANARY /var/lib/docker/harpoon-mgmt-canary" "$DISK" >/dev/null || fail "cannot write Docker canary"
# Normalize metadata after deliberate offline debugfs mutations before boot.
set +e
"$E2FSCK" -fy "$DISK" >/dev/null
fsck_status=$?
set -e
[ "$fsck_status" -le 1 ] || fail "fixture e2fsck status $fsck_status"
if stat_path /usr/local/bin/harpoon-mgmt | grep -q 'Inode:'; then fail "old root still has harpoon-mgmt"; fi
for path in "${NAT_MODULES[@]}"; do if stat_path "$path" | grep -q 'Inode:'; then fail "old root still has $path"; fi; done
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-before" "$DISK" >/dev/null || fail "fixture Docker canary missing before boot"
cmp -s "$CANARY" "$TMP/canary-before" || fail "fixture Docker canary differs before boot"
before_identity=$(stat -f '%d:%i:%z' "$DISK")
before_uuid=$("$DUMPE2FS" -h "$DISK" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')
check test -n "$before_uuid"

stop_fixture
HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$DISK" HARPOON_INITRAMFS="$INITRAMFS" "$BIN" start || fail "first boot did not start"
HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- true || fail "management exec true"
uname_output=$(HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- uname -a) || fail "management exec uname"
[[ "$uname_output" == *Linux* ]] || fail "management exec uname"
for module in xt_nat xt_REDIRECT xt_MASQUERADE; do HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- modprobe "$module" || fail "management modprobe $module"; done
for _ in $(seq 1 30); do
  docker --context harpoon version >/dev/null 2>&1 && break
  sleep 1
done
docker --context harpoon version >/dev/null 2>&1 || fail "Docker is not ready"
for marker in \
  'HARPOON_MGMT_WRAPPER_START' \
  'HARPOON_MGMT_WRAPPER_TARGET exists=yes regular=yes executable=yes' \
  'HARPOON_MGMT_WRAPPER_INTERPRETER exists=yes executable=yes' \
  'HARPOON_MGMT_CHILD_START' \
  'HARPOON_MGMT_RESPONSE_SENT status=0' \
  'HARPOON_MGMT_CHILD_EXIT status=0'; do
  grep -q "$marker" /tmp/harpoon-serial.log || fail "missing serial marker: $marker"
done
stop_fixture

echo "mgmt-migration: checking in-place identity"
after_identity=$(stat -f '%d:%i:%z' "$DISK")
after_uuid=$("$DUMPE2FS" -h "$DISK" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')
[ "$before_identity" = "$after_identity" ] || fail "disk image was replaced"
[ "$before_uuid" = "$after_uuid" ] || fail "filesystem UUID changed"
echo "mgmt-migration: checking reconciled modes"
for path in /usr/local/bin/harpoon-mgmt /usr/local/bin/harpoon-mgmt-wrapper; do
  mode_owner "$path" || fail "bad mode or owner: $path"
done
for path in /usr/bin/python3 /usr/bin/socat; do
  mode_exec "$path" || fail "not executable: $path"
done
echo "mgmt-migration: checking payload and Docker canary"
for path in /usr/local/bin/harpoon-mgmt /usr/local/bin/harpoon-mgmt-wrapper; do
  dump="$TMP/$(basename "$path")"
  "$DEBUGFS" -R "dump $path $dump" "$DISK" >/dev/null || fail "cannot dump payload: $path"
  source="$REPO_ROOT/tools/guest-builder/src/$(basename "$path")"
  [ "$(shasum -a 256 "$dump" | cut -d' ' -f1)" = "$(shasum -a 256 "$source" | cut -d' ' -f1)" ] || fail "payload hash mismatch: $path"
done
for path in "${NAT_MODULES[@]}" "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias" /usr/local/share/harpoon-runtime/modules.manifest; do disk_matches_payload "$path" || fail "module payload mismatch: $path"; done
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-after" "$DISK" >/dev/null
cmp -s "$CANARY" "$TMP/canary-after" || fail "Docker canary changed"
echo "mgmt-migration: checking second boot idempotence"
HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$DISK" HARPOON_INITRAMFS="$INITRAMFS" "$BIN" start || fail "second boot did not start"
HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- true || fail "second boot management exec true"
for module in xt_nat xt_REDIRECT xt_MASQUERADE; do HARPOON_TEST_TMPDIR="$TMP/runtime" HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- modprobe "$module" || fail "second boot modprobe $module"; done
docker --context harpoon version >/dev/null 2>&1 || fail "second boot Docker is not ready"
stop_fixture
for path in "${NAT_MODULES[@]}" "$MODULE_DIR/modules.dep" "$MODULE_DIR/modules.alias" /usr/local/share/harpoon-runtime/modules.manifest; do disk_matches_payload "$path" || fail "second boot module payload mismatch: $path"; done
"$DEBUGFS" -R "dump /var/lib/docker/harpoon-mgmt-canary $TMP/canary-second" "$DISK" >/dev/null
cmp -s "$CANARY" "$TMP/canary-second" || fail "second boot Docker canary changed"
completed=1
echo "mgmt-migration PASS: old root reconciled in place; Docker canary, module closure, and management handoff preserved"
