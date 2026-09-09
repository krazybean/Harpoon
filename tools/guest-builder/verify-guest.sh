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
HARPOON_MGMT_WRAPPER="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt-wrapper"
REQUIRED_MODULES="$REPO_ROOT/tools/guest-builder/required-modules.txt"
REQUIRED_FEATURES="$REPO_ROOT/tools/guest-builder/required-kernel-features.txt"
FEATURE_MODULES="$REPO_ROOT/tools/guest-builder/kernel-feature-modules.txt"

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
check "harpoon-mgmt wrapper source exists" test -f "$HARPOON_MGMT_WRAPPER"
check "init source exists" test -f "$INIT_SRC"
if "$SCRIPT_DIR/build-initramfs.sh" --check >/dev/null 2>&1; then echo "[verify-guest] PASS: initramfs input fingerprint current" >&2; else echo "[verify-guest] FAIL: initramfs input fingerprint stale" >&2; FAIL=1; fi

# 2. Root template logical size exactly 2147483648
if [ -f "$ROOT_IMG" ]; then
  SZ=$(stat -f%z "$ROOT_IMG" 2>/dev/null || stat -c%s "$ROOT_IMG")
  if [ "$SZ" = "2147483648" ]; then echo "[verify-guest] PASS: root logical 2G" >&2; else echo "[verify-guest] FAIL: root logical $SZ != 2147483648" >&2; FAIL=1; fi
fi

# 3. Initramfs content checks via cpio listing
if [ -f "$INITRAMFS" ]; then
  LISTING=$(gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null || echo "")
  grep -q "usr/local/bin/harpoon-mgmt" <<< "$LISTING" && echo "[verify-guest] PASS: harpoon-mgmt in initramfs" >&2 || { echo "[verify-guest] FAIL: harpoon-mgmt missing in initramfs" >&2; FAIL=1; }
  grep -q "usr/local/bin/harpoon-mgmt-wrapper" <<< "$LISTING" && echo "[verify-guest] PASS: harpoon-mgmt wrapper in initramfs" >&2 || { echo "[verify-guest] FAIL: harpoon-mgmt wrapper missing in initramfs" >&2; FAIL=1; }
  grep -q "lib/modules.*ext4.ko" <<< "$LISTING" && echo "[verify-guest] PASS: ext4.ko in initramfs" >&2 || { echo "[verify-guest] FAIL: ext4.ko missing" >&2; FAIL=1; }
  GUEST_TMPDIR=$(mktemp -d)
  gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$GUEST_TMPDIR" && cpio -idm 2>/dev/null || true)
  RUNTIME_MANIFEST="$GUEST_TMPDIR/usr/local/share/harpoon-runtime/python.manifest"
  if [ -f "$RUNTIME_MANIFEST" ]; then
    RUNTIME_OK=1
    while read -r kind value mode path; do
      case "$kind" in
        F) [ -f "$GUEST_TMPDIR/$path" ] && [ "$(shasum -a 256 "$GUEST_TMPDIR/$path" | cut -d' ' -f1)" = "$value" ] || RUNTIME_OK=0 ;;
        L) [ -L "$GUEST_TMPDIR/$mode" ] && [ "$(readlink "$GUEST_TMPDIR/$mode")" = "$value" ] || RUNTIME_OK=0 ;;
        *) RUNTIME_OK=0 ;;
      esac
    done < "$RUNTIME_MANIFEST"
    [ "$RUNTIME_OK" -eq 1 ] && echo "[verify-guest] PASS: Python runtime manifest matches initramfs" >&2 || { echo "[verify-guest] FAIL: Python runtime manifest mismatch" >&2; FAIL=1; }
    [ "$(readlink "$GUEST_TMPDIR/usr/bin/python3" 2>/dev/null)" = "python3.12" ] && echo "[verify-guest] PASS: Python symlink target" >&2 || { echo "[verify-guest] FAIL: Python symlink target" >&2; FAIL=1; }
    file "$GUEST_TMPDIR/usr/bin/python3.12" 2>/dev/null | grep -q aarch64 && echo "[verify-guest] PASS: initramfs Python aarch64 ELF" >&2 || { echo "[verify-guest] FAIL: initramfs Python is not aarch64 ELF" >&2; FAIL=1; }
  else
    echo "[verify-guest] FAIL: Python runtime manifest missing in initramfs" >&2; FAIL=1
  fi
  MODULE_DIR=$(find "$GUEST_TMPDIR/lib/modules" -mindepth 1 -maxdepth 1 -type d | head -n1)
  CHECKED="$GUEST_TMPDIR/.checked-modules"
  verify_path() {
    local path="$1" dep
    grep -qxF "$path" "$CHECKED" 2>/dev/null && return
    echo "$path" >> "$CHECKED"
    [ -f "$MODULE_DIR/$path" ] || { echo "[verify-guest] FAIL: module missing $path" >&2; FAIL=1; return; }
    for dep in $(awk -v path="$path" '$1==path ":" {for(i=2;i<=NF;i++) print $i}' "$MODULE_DIR/modules.dep"); do verify_path "$dep"; done
  }
  resolve_module() {
    local name="$1" path alias
    path=$(find "$MODULE_DIR" -type f -name "$name.ko" | sed "s#^$MODULE_DIR/##" | LC_ALL=C sort | head -n1)
    if [ -z "$path" ]; then
      alias=$(awk -v name="$name" '$1=="alias" && $2==name {print $3; exit}' "$MODULE_DIR/modules.alias")
      [ -n "$alias" ] && path=$(find "$MODULE_DIR" -type f -name "$alias.ko" | sed "s#^$MODULE_DIR/##" | LC_ALL=C sort | head -n1)
    fi
    [ -n "$path" ] || return 1
    echo "$path"
  }
  verify_module() {
    local path
    path=$(resolve_module "$1") || { echo "[verify-guest] FAIL: unresolved module $1" >&2; FAIL=1; return; }
    verify_path "$path"
  }
  while IFS= read -r mod; do case "$mod" in ''|'#'*) continue ;; esac; verify_module "$mod"; done < "$REQUIRED_MODULES"
  echo "[verify-guest] PASS: required module closure" >&2
  while IFS= read -r feature; do
    case "$feature" in ''|'#'*) continue ;; esac
    mod=$(awk -v feature="$feature" '$1==feature {print $2; exit}' "$FEATURE_MODULES")
    if [ -z "$mod" ]; then
      echo "[verify-guest] FAIL: required kernel feature $feature has no module mapping" >&2; FAIL=1
    elif grep -q "/$mod\\.ko$" "$MODULE_DIR/modules.builtin"; then
      echo "[verify-guest] PASS: required kernel feature $feature=y" >&2
    elif path=$(resolve_module "$mod"); then
      verify_path "$path"
      echo "[verify-guest] PASS: required kernel feature $feature=m ($mod)" >&2
    else
      echo "[verify-guest] FAIL: required kernel feature $feature is unset" >&2; FAIL=1
    fi
  done < "$REQUIRED_FEATURES"
  rm -rf "$GUEST_TMPDIR"
  grep -q "lib/modules.*virtio_blk.ko" <<< "$LISTING" && echo "[verify-guest] PASS: virtio_blk.ko present" >&2 || { echo "[verify-guest] FAIL: virtio_blk.ko missing" >&2; FAIL=1; }
  grep -q "lib/modules.*vsock.ko" <<< "$LISTING" && echo "[verify-guest] PASS: vsock.ko present" >&2 || { echo "[verify-guest] FAIL: vsock.ko missing" >&2; FAIL=1; }
  grep -q "lib/modules.*vmw_vsock" <<< "$LISTING" && echo "[verify-guest] PASS: vmw_vsock modules present" >&2 || { echo "[verify-guest] FAIL: vmw_vsock modules missing" >&2; FAIL=1; }
  grep -q "lib/modules.*virtiofs.ko" <<< "$LISTING" && echo "[verify-guest] PASS: virtiofs.ko present" >&2 || { echo "[verify-guest] FAIL: virtiofs.ko missing" >&2; FAIL=1; }
  grep -q "sbin/apk" <<< "$LISTING" && echo "[verify-guest] PASS: apk present" >&2 || { echo "[verify-guest] FAIL: apk missing" >&2; FAIL=1; }
  # Artifact-level: resize2fs must be present in initramfs (offline, not via apk)
  grep -q "sbin/resize2fs" <<< "$LISTING" && echo "[verify-guest] PASS: resize2fs in initramfs (offline)" >&2 || { echo "[verify-guest] FAIL: resize2fs missing in initramfs" >&2; FAIL=1; }
  grep -q "usr/sbin/resize2fs" <<< "$LISTING" && echo "[verify-guest] PASS: resize2fs in usr/sbin" >&2 || { echo "[verify-guest] FAIL: resize2fs missing in usr/sbin" >&2; FAIL=1; }
  grep -q "libext2fs" <<< "$LISTING" && echo "[verify-guest] PASS: libext2fs in initramfs" >&2 || { echo "[verify-guest] FAIL: libext2fs missing in initramfs" >&2; FAIL=1; }
  grep -q "libblkid" <<< "$LISTING" && echo "[verify-guest] PASS: libblkid in initramfs" >&2 || { echo "[verify-guest] FAIL: libblkid missing in initramfs" >&2; FAIL=1; }
  # Verify initramfs init will be able to run resize2fs: check it contains the refresh logic
  if grep -q "HARPOON_RESIZE2FS_REFRESH" "$INIT_SRC"; then echo "[verify-guest] PASS: init has resize2fs refresh to final root" >&2; else echo "[verify-guest] FAIL: init missing resize2fs refresh" >&2; FAIL=1; fi
fi

# 4. Init source checks — the class of defect that shipped RC
if [ -f "$INIT_SRC" ]; then
  # init must provide offline resize2fs via initramfs (not rely on apk at boot)
  if grep -q "apk add.*e2fsprogs" "$INIT_SRC"; then echo "[verify-guest] FAIL: init still apk adds e2fsprogs (should be offline via initramfs)" >&2; FAIL=1; else echo "[verify-guest] PASS: init does not apk add e2fsprogs (offline)" >&2; fi
  # init must verify resize2fs (not e2fsprogs binary)
  if grep -q 'for bin in.*resize2fs' "$INIT_SRC"; then echo "[verify-guest] PASS: init checks resize2fs binary" >&2; else echo "[verify-guest] FAIL: init does not check resize2fs" >&2; FAIL=1; fi
  if grep -q 'for bin in.*e2fsprogs' "$INIT_SRC"; then echo "[verify-guest] FAIL: init still checks e2fsprogs (bug)" >&2; FAIL=1; else echo "[verify-guest] PASS: init does not check e2fsprogs binary" >&2; fi
  # init must do disk resize BEFORE docker AND before APK (offline)
  # Find line numbers: DISK_CHECK_START should appear before DOCKERD_START and before APK
  DISK_LINE=$(grep -n "HARPOON_DISK_CHECK_START" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  DOCKER_LINE=$(grep -n "HARPOON_DOCKERD_START\|dockerd --host" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 0)
  APK_LINE=$(grep -n "HARPOON_APK_UPDATE_START\|apk update" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  if [ "$DISK_LINE" -lt "$DOCKER_LINE" ] && [ "$DISK_LINE" -ne 9999 ]; then echo "[verify-guest] PASS: disk resize before Docker ($DISK_LINE < $DOCKER_LINE)" >&2; else echo "[verify-guest] FAIL: disk resize not before Docker (disk:$DISK_LINE docker:$DOCKER_LINE)" >&2; FAIL=1; fi
  if [ "$DISK_LINE" -lt "$APK_LINE" ] && [ "$DISK_LINE" -ne 9999 ]; then echo "[verify-guest] PASS: disk resize before APK ($DISK_LINE < $APK_LINE) offline" >&2; else echo "[verify-guest] FAIL: disk resize not before APK (disk:$DISK_LINE apk:$APK_LINE) — must be offline" >&2; FAIL=1; fi
  # init must include failure handling for resize
  if grep -q "HARPOON_DISK_RESIZE_FAILED" "$INIT_SRC"; then echo "[verify-guest] PASS: resize failure handling" >&2; else echo "[verify-guest] FAIL: no resize failure handling" >&2; FAIL=1; fi
  # init must contain harpoon-mgmt startup with retry
  if grep -q "harpoon-mgmt" "$INIT_SRC" && grep -q "HARPOON_MGMT_READY" "$INIT_SRC"; then echo "[verify-guest] PASS: mgmt startup in init" >&2; else echo "[verify-guest] FAIL: mgmt startup missing" >&2; FAIL=1; fi
  if grep -q "EXEC:/usr/local/bin/harpoon-mgmt-wrapper" "$INIT_SRC"; then echo "[verify-guest] PASS: mgmt listener uses wrapper" >&2; else echo "[verify-guest] FAIL: mgmt listener bypasses wrapper" >&2; FAIL=1; fi
  if grep -q 'mount -t devpts devpts /dev/pts' "$INIT_SRC"; then echo "[verify-guest] PASS: devpts mounted for management shell" >&2; else echo "[verify-guest] FAIL: devpts mount missing" >&2; FAIL=1; fi
  NET_LINE=$(grep -n '^NET_MODULES=' "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  for mod in af_packet nfnetlink nf_tables nft_compat nft_chain_nat xt_nat xt_REDIRECT xt_MASQUERADE; do
    grep -Eq "NET_MODULES=.*(^|[[:space:]])$mod([[:space:]]|\")" "$INIT_SRC" && echo "[verify-guest] PASS: iptables-nft DNAT activation $mod" >&2 || { echo "[verify-guest] FAIL: iptables-nft DNAT activation missing $mod" >&2; FAIL=1; }
  done
  if [ "$NET_LINE" -lt "$DOCKER_LINE" ]; then echo "[verify-guest] PASS: iptables-nft DNAT activation before Docker" >&2; else echo "[verify-guest] FAIL: iptables-nft DNAT activation not before Docker" >&2; FAIL=1; fi
  # Verify repacked initramfs matches source
  GUEST_TMPDIR=$(mktemp -d)
  gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$GUEST_TMPDIR" && cpio -idm 2>/dev/null || true)
  if [ -f "$GUEST_TMPDIR/init" ] && diff -q "$INIT_SRC" "$GUEST_TMPDIR/init" >/dev/null 2>&1; then
    echo "[verify-guest] PASS: initramfs init matches src/init" >&2
  else
    echo "[verify-guest] FAIL: initramfs init differs from src/init — rebuild required" >&2; FAIL=1
  fi
  rm -rf "$GUEST_TMPDIR"
fi

# 5. harpoon-mgmt must be valid python
if [ -f "$HARPOON_MGMT" ]; then
  if python3 -c "import ast; ast.parse(open('$HARPOON_MGMT').read())" 2>&1 | head -n 5; then echo "[verify-guest] PASS: harpoon-mgmt py_compile" >&2; else echo "[verify-guest] FAIL: harpoon-mgmt py_compile" >&2; FAIL=1; fi
  if head -n1 "$HARPOON_MGMT" | grep -q "python3"; then echo "[verify-guest] PASS: harpoon-mgmt shebang python3" >&2; else echo "[verify-guest] FAIL: harpoon-mgmt shebang" >&2; FAIL=1; fi
fi

# 6. RuntimeConfig defaults
if grep -q "defaultProvisionBytes.*32.*GiB" "$REPO_ROOT/harpoon/Sources/RuntimeConfig.swift" 2>/dev/null; then echo "[verify-guest] PASS: default 32G" >&2; else echo "[verify-guest] FAIL: default not 32G" >&2; FAIL=1; fi

if [ $FAIL -ne 0 ]; then
  echo "[verify-guest] FAIL: canonical guest verification failed" >&2
  exit 1
fi
echo "[verify-guest] PASS: all checks passed" >&2
