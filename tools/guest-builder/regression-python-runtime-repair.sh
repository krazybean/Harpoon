#!/bin/bash
set -euo pipefail

# Proves initramfs reconciliation repairs only the immutable management Python runtime.
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$REPO_ROOT/harpoon/build/harpoon"
ROOT="$REPO_ROOT/assets/guest/harpoon-root.img"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
DEBUGFS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs
DUMPE2FS=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/dumpe2fs
E2FSCK=/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/e2fsck
TMP=$(mktemp -d /tmp/harpoon-python-runtime.XXXXXX)
CANARY="$TMP/docker-canary"
EXPECTED=bef4c5021fabedfdf3144e49373203dd1c577628af558382807f705a86bd1300

fail() { echo "python-runtime-repair FAIL: $*" >&2; exit 1; }
check() { "$@" || fail "$*"; }
stop() {
  HARPOON_TEST_TMPDIR="$1/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" stop >/dev/null 2>&1 || true
  [ -f "$1/runtime/runtime.pid" ] && kill "$(cat "$1/runtime/runtime.pid")" 2>/dev/null || true
  return 0
}
trap 'for d in "$TMP"/*; do [ -d "$d" ] && stop "$d"; done; rm -rf "$TMP"' EXIT
dump() { "$DEBUGFS" -c -R "dump $1 $2" "$3" >/dev/null; }
link_target() { "$DEBUGFS" -c -R "stat $1" "$2" 2>/dev/null | sed -n 's/.*Fast link dest: "\(.*\)"/\1/p'; }
hash_path() { dump "$1" "$TMP/hash" "$2"; shasum -a 256 "$TMP/hash" | cut -d' ' -f1; }
docker_ready() {
  for _ in $(seq 1 30); do
    DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version >/dev/null 2>&1 && return 0
    sleep 1
  done
  fail "Docker did not become ready"
}

check test -x "$BIN"
check test -f "$ROOT"
check test -f "$INITRAMFS"
check test -x "$DEBUGFS"
check test -x "$DUMPE2FS"
check test -x "$E2FSCK"
printf 'harpoon-python-runtime-docker-canary\n' > "$CANARY"

run_case() {
  local name="$1" before_uuid before_identity first_hash second_hash volume="harpoon-python-runtime-$1"
  local disk="$TMP/$name/root.img" runtime="$TMP/$name"
  mkdir -p "$TMP/$name"
  cp -c "$ROOT" "$disk" 2>/dev/null || cp "$ROOT" "$disk"
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$disk" HARPOON_KERNEL="$REPO_ROOT/assets/guest/Image-virt" HARPOON_INITRAMFS="$INITRAMFS" "$BIN" start
  docker_ready
  DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker volume create "$volume" >/dev/null
  DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker run --rm -v "$volume:/data" alpine:3.22 sh -c 'printf "harpoon-python-runtime-docker-canary\\n" > /data/canary'
  stop "$runtime"
  case "$name" in
    zero-byte)
      : > "$TMP/$name/empty"
      "$DEBUGFS" -w -R 'rm /usr/bin/python3.12' "$disk" >/dev/null
      "$DEBUGFS" -w -R "write $TMP/$name/empty /usr/bin/python3.12" "$disk" >/dev/null
      ;;
    missing-symlink)
      "$DEBUGFS" -w -R 'rm /usr/bin/python3' "$disk" >/dev/null
      ;;
    wrong-target)
      "$DEBUGFS" -w -R 'rm /usr/bin/python3' "$disk" >/dev/null
      "$DEBUGFS" -w -R 'symlink /usr/bin/python3 not-python3.12' "$disk" >/dev/null
      ;;
    wrong-hash)
      printf 'not-a-python-elf\n' > "$TMP/$name/bad"
      "$DEBUGFS" -w -R 'rm /usr/bin/python3.12' "$disk" >/dev/null
      "$DEBUGFS" -w -R "write $TMP/$name/bad /usr/bin/python3.12" "$disk" >/dev/null
      ;;
  esac
  # debugfs deliberately makes the corruption offline; normalize ext4 metadata
  # before the guest mounts the image, without changing guest Docker data.
  set +e
  "$E2FSCK" -fy "$disk" >/dev/null
  fsck_status=$?
  set -e
  [ "$fsck_status" -le 1 ] || fail "$name e2fsck status $fsck_status"
  before_identity=$(stat -f '%d:%i:%z' "$disk")
  before_uuid=$("$DUMPE2FS" -h "$disk" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$disk" HARPOON_KERNEL="$REPO_ROOT/assets/guest/Image-virt" HARPOON_INITRAMFS="$INITRAMFS" "$BIN" start
  docker_ready
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- python3 -c 'import fcntl,json,os,pty,select,signal,struct,subprocess,sys,termios'
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- true
  DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker run --rm -v "$volume:/data:ro" alpine:3.22 cat /data/canary | cmp -s "$CANARY" - || fail "$name changed Docker canary"
  stop "$runtime"
  [ "$(hash_path /usr/bin/python3.12 "$disk")" = "$EXPECTED" ] || fail "$name did not restore python3.12"
  [ "$(link_target /usr/bin/python3 "$disk")" = python3.12 ] || fail "$name did not restore python3 symlink"
  [ "$before_identity" = "$(stat -f '%d:%i:%z' "$disk")" ] || fail "$name replaced disk image"
  [ "$before_uuid" = "$("$DUMPE2FS" -h "$disk" 2>/dev/null | awk -F': ' '/Filesystem UUID:/{print $2}')" ] || fail "$name changed filesystem UUID"
  first_hash=$(hash_path /usr/bin/python3.12 "$disk")
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 HARPOON_DISK="$disk" HARPOON_KERNEL="$REPO_ROOT/assets/guest/Image-virt" HARPOON_INITRAMFS="$INITRAMFS" "$BIN" start
  docker_ready
  HARPOON_TEST_TMPDIR="$runtime/runtime" HARPOON_TEST_MODE=1 HARPOON_ALLOW_TMP_FALLBACK=1 "$BIN" exec -- true
  DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker run --rm -v "$volume:/data:ro" alpine:3.22 cat /data/canary | cmp -s "$CANARY" - || fail "$name changed Docker canary on second boot"
  stop "$runtime"
  second_hash=$(hash_path /usr/bin/python3.12 "$disk")
  [ "$first_hash" = "$second_hash" ] || fail "$name second reconciliation changed Python"
  echo "python-runtime-repair PASS: $name"
}

for case_name in zero-byte missing-symlink wrong-target wrong-hash; do run_case "$case_name"; done
