#!/bin/bash
set -euo pipefail
# ponytail: verify canonical guest contains mandatory runtime components — fails release if any missing
# Checks:
# 1. initramfs contains harpoon-mgmt + required kernel modules (ext4, vsock, virtio)
# 2. init script references resize2fs (not e2fsprogs) and does disk resize BEFORE docker
# 3. init installs e2fsprogs package (for resize2fs binary)
# 4. bundled root template size is exactly 2G sparse
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
INIT_SRC="$REPO_ROOT/tools/guest-builder/src/init"
ROOT_IMG="$REPO_ROOT/assets/guest/harpoon-root.img"
HARPOON_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"

FAIL=0
check() {
  local msg="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "[verify-guest] PASS: $msg" >&2; else echo "[verify-guest] FAIL: $msg" >&2; FAIL=1; fi
}
check_not() {
  local msg="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "[verify-guest] FAIL: $msg (should not exist)" >&2; FAIL=1; else echo "[verify-guest] PASS: $msg absent" >&2; fi
}

echo "[verify-guest] verifying canonical guest..." >&2

# 1. Files exist
check "initramfs exists" test -f "$INITRAMFS"
check "root template exists" test -f "$ROOT_IMG"
check "harpoon-mgmt source exists" test -f "$HARPOON_MGMT"
check "init source exists" test -f "$INIT_SRC"

# 2. Root template logical size exactly 2147483648
if [ -f "$ROOT_IMG" ]; then
  SZ=$(stat -f%z "$ROOT_IMG" 2>/dev/null || stat -c%s "$ROOT_IMG")
  if [ "$SZ" = "2147483648" ]; then echo "[verify-guest] PASS: root logical 2G" >&2; else echo "[verify-guest] FAIL: root logical $SZ != 2147483648" >&2; FAIL=1; fi
fi

# 3. Initramfs content checks via cpio listing
if [ -f "$INITRAMFS" ]; then
  LISTING=$(gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null || echo "")
  echo "$LISTING" | grep -q "usr/local/bin/harpoon-mgmt" && echo "[verify-guest] PASS: harpoon-mgmt in initramfs" >&2 || { echo "[verify-guest] FAIL: harpoon-mgmt missing in initramfs" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "lib/modules.*ext4.ko" && echo "[verify-guest] PASS: ext4.ko in initramfs" >&2 || { echo "[verify-guest] FAIL: ext4.ko missing" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "lib/modules.*virtio_blk.ko" && echo "[verify-guest] PASS: virtio_blk.ko present" >&2 || { echo "[verify-guest] FAIL: virtio_blk.ko missing" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "lib/modules.*vsock.ko" && echo "[verify-guest] PASS: vsock.ko present" >&2 || { echo "[verify-guest] FAIL: vsock.ko missing" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "lib/modules.*vmw_vsock" && echo "[verify-guest] PASS: vmw_vsock modules present" >&2 || { echo "[verify-guest] FAIL: vmw_vsock modules missing" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "lib/modules.*virtiofs.ko" && echo "[verify-guest] PASS: virtiofs.ko present" >&2 || { echo "[verify-guest] FAIL: virtiofs.ko missing" >&2; FAIL=1; }
  echo "$LISTING" | grep -q "sbin/apk" && echo "[verify-guest] PASS: apk present" >&2 || { echo "[verify-guest] FAIL: apk missing" >&2; FAIL=1; }
fi

# 4. Init source checks — the class of defect that shipped RC
if [ -f "$INIT_SRC" ]; then
  # init must install e2fsprogs package
  if grep -q "apk add.*e2fsprogs" "$INIT_SRC"; then echo "[verify-guest] PASS: init installs e2fsprogs" >&2; else echo "[verify-guest] FAIL: init does not apk add e2fsprogs" >&2; FAIL=1; fi
  # init must verify resize2fs (not e2fsprogs binary)
  if grep -q 'for bin in.*resize2fs' "$INIT_SRC"; then echo "[verify-guest] PASS: init checks resize2fs binary" >&2; else echo "[verify-guest] FAIL: init does not check resize2fs" >&2; FAIL=1; fi
  if grep -q 'for bin in.*e2fsprogs' "$INIT_SRC"; then echo "[verify-guest] FAIL: init still checks e2fsprogs (bug)" >&2; FAIL=1; else echo "[verify-guest] PASS: init does not check e2fsprogs binary" >&2; fi
  # init must do disk resize BEFORE docker
  # Find line numbers: DISK_CHECK_START should appear before DOCKERD_START
  DISK_LINE=$(grep -n "HARPOON_DISK_CHECK_START" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  DOCKER_LINE=$(grep -n "HARPOON_DOCKERD_START\|dockerd --host" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 0)
  if [ "$DISK_LINE" -lt "$DOCKER_LINE" ] && [ "$DISK_LINE" -ne 9999 ]; then echo "[verify-guest] PASS: disk resize before Docker ($DISK_LINE < $DOCKER_LINE)" >&2; else echo "[verify-guest] FAIL: disk resize not before Docker (disk:$DISK_LINE docker:$DOCKER_LINE)" >&2; FAIL=1; fi
  # init must include failure handling for resize
  if grep -q "HARPOON_DISK_RESIZE_FAILED" "$INIT_SRC"; then echo "[verify-guest] PASS: resize failure handling" >&2; else echo "[verify-guest] FAIL: no resize failure handling" >&2; FAIL=1; fi
  # init must contain harpoon-mgmt startup with retry
  if grep -q "harpoon-mgmt" "$INIT_SRC" && grep -q "HARPOON_MGMT_READY" "$INIT_SRC"; then echo "[verify-guest] PASS: mgmt startup in init" >&2; else echo "[verify-guest] FAIL: mgmt startup missing" >&2; FAIL=1; fi
  # Verify repacked initramfs matches source
  TMPDIR=$(mktemp -d)
  gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$TMPDIR" && cpio -idm 2>/dev/null || true)
  if [ -f "$TMPDIR/init" ] && diff -q "$INIT_SRC" "$TMPDIR/init" >/dev/null 2>&1; then
    echo "[verify-guest] PASS: initramfs init matches src/init" >&2
  else
    echo "[verify-guest] FAIL: initramfs init differs from src/init — rebuild required" >&2; FAIL=1
  fi
  rm -rf "$TMPDIR"
fi

# 5. harpoon-mgmt must be valid python
if [ -f "$HARPOON_MGMT" ]; then
  if python3 -m py_compile "$HARPOON_MGMT" 2>&1 | head -n 5; then echo "[verify-guest] PASS: harpoon-mgmt py_compile" >&2; else echo "[verify-guest] FAIL: harpoon-mgmt py_compile" >&2; FAIL=1; fi
  if head -n1 "$HARPOON_MGMT" | grep -q "python3"; then echo "[verify-guest] PASS: harpoon-mgmt shebang python3" >&2; else echo "[verify-guest] FAIL: harpoon-mgmt shebang" >&2; FAIL=1; fi
fi

# 6. RuntimeConfig defaults
if grep -q "defaultProvisionBytes.*32.*GiB" "$REPO_ROOT/harpoon/Sources/RuntimeConfig.swift" 2>/dev/null; then echo "[verify-guest] PASS: default 32G" >&2; else echo "[verify-guest] FAIL: default not 32G" >&2; FAIL=1; fi

if [ $FAIL -ne 0 ]; then
  echo "[verify-guest] FAIL: canonical guest verification failed" >&2
  exit 1
fi
echo "[verify-guest] PASS: all checks passed" >&2
