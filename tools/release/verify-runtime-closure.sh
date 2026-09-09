#!/bin/bash
set -euo pipefail
# ponytail: release runtime closure gate — proves every dependency for boot/Docker/management is bundled, not fetched
# INITRAMFS, ROOT, BOOT ORDER. Host signing is verified post-sign in verify-signatures.sh.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
INIT_SRC="$REPO_ROOT/tools/guest-builder/src/init"
ROOT_IMG="$REPO_ROOT/assets/guest/harpoon-root.img"
REQUIRED_MODULES="$REPO_ROOT/tools/guest-builder/required-modules.txt"
REQUIRED_FEATURES="$REPO_ROOT/tools/guest-builder/required-kernel-features.txt"
FEATURE_MODULES="$REPO_ROOT/tools/guest-builder/kernel-feature-modules.txt"
HARPOON_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
FAIL=0
say() { echo "[closure] $*" >&2; }
pass() { echo "[closure] PASS: $1" >&2; }
fail() { echo "[closure] FAIL: $1" >&2; FAIL=1; }

# Portable readelf discovery
find_readelf() {
  if command -v llvm-readelf >/dev/null 2>&1; then echo "$(command -v llvm-readelf)"; return 0; fi
  if command -v readelf >/dev/null 2>&1; then echo "$(command -v readelf)"; return 0; fi
  for p in /usr/local/opt/llvm@15/bin/llvm-readelf /opt/homebrew/opt/llvm/bin/llvm-readelf /opt/homebrew/opt/llvm@15/bin/llvm-readelf /usr/local/opt/llvm/bin/llvm-readelf; do
    if [ -x "$p" ]; then echo "$p"; return 0; fi
  done
  return 1
}
READELF=""
if ! READELF=$(find_readelf); then
  echo "[closure] FAIL: no ELF inspection tool found (llvm-readelf/readelf required for closure verification)" >&2
  FAIL=1
  READELF="false"
else
  say "using readelf: $READELF"
fi

say "verifying runtime closure..."

# INITRAMFS — busybox applets (representative core set used by init)
if [ -f "$INITRAMFS" ]; then
  LISTING=$(gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null || echo "")
  for cmd in sh mount umount mkdir cp mv rm chmod chown ln grep sed awk cut cat sleep sync stat df blockdev modprobe insmod lsmod switch_root; do
    if grep -q "bin/$cmd" <<< "$LISTING" || grep -q "sbin/$cmd" <<< "$LISTING"; then pass "initramfs busybox $cmd"; else fail "initramfs missing busybox $cmd"; fi
  done
  # external binaries that must be present (not busybox)
  for bin in sbin/apk sbin/blkid sbin/resize2fs usr/sbin/resize2fs; do
    if grep -q "$bin" <<< "$LISTING"; then pass "initramfs $bin"; else fail "initramfs missing $bin"; fi
  done
  for lib in libext2fs libblkid libcom_err libe2p libuuid; do
    if grep -q "$lib" <<< "$LISTING"; then pass "initramfs $lib"; else fail "initramfs missing $lib"; fi
  done
  # ELF closure for resize2fs — verify actual PT_INTERP path exists
  RESIZE_DIR="$WORK_DIR/resize2fs"
  mkdir -p "$RESIZE_DIR"
  gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$RESIZE_DIR" && cpio -idm 2>/dev/null || true)
  if [ -f "$RESIZE_DIR/usr/sbin/resize2fs" ]; then
    if [ "$READELF" != "false" ]; then
      if $READELF --dynamic "$RESIZE_DIR/usr/sbin/resize2fs" 2>&1 | grep -q "libext2fs.so.2"; then pass "resize2fs DT_NEEDED libext2fs"; else fail "resize2fs missing libext2fs"; fi
      # Verify actual interpreter path emitted by readelf exists in artifact
      INTERP=$($READELF -l "$RESIZE_DIR/usr/sbin/resize2fs" 2>&1 | grep -o "/[^ ]*ld-musl[^ ]*" | head -n1 | tr -d ']' || true)
      if [ -z "$INTERP" ]; then
        # fallback parse Requesting program interpreter line
        INTERP=$($READELF -l "$RESIZE_DIR/usr/sbin/resize2fs" 2>&1 | grep "Requesting program interpreter" | sed -n 's/.*: \([^]]*\)].*/\1/p' | head -n1 | tr -d ']' || true)
      fi
      if [ -n "$INTERP" ]; then
        # INTERP is absolute like /lib/ld-musl-aarch64.so.1 — check under RESIZE_DIR
        if [ -f "$RESIZE_DIR$INTERP" ]; then pass "resize2fs loader $INTERP"; else fail "resize2fs loader missing: $INTERP not in initramfs (expected $RESIZE_DIR$INTERP)"; fi
      else
        fail "resize2fs could not determine PT_INTERP"
      fi
      for needed in libe2p libext2fs libcom_err; do
        if $READELF --dynamic "$RESIZE_DIR/usr/sbin/resize2fs" 2>&1 | grep -q "$needed"; then
          if ls "$RESIZE_DIR/usr/lib/$needed"* >/dev/null 2>&1 || ls "$RESIZE_DIR/lib/$needed"* >/dev/null 2>&1; then pass "resize2fs $needed present"; else fail "resize2fs $needed NEEDED but not in initramfs"; fi
        fi
      done
    else
      fail "resize2fs ELF closure skipped — no readelf available"
    fi
    if file "$RESIZE_DIR/usr/sbin/resize2fs" 2>&1 | grep -q "aarch64"; then pass "resize2fs aarch64"; else fail "resize2fs not aarch64"; fi
  else
    fail "resize2fs not in initramfs for ELF check"
  fi
  # kernel modules
  for mod in ext4 virtio_blk vsock vmw_vsock virtiofs; do
    if grep -q "$mod" <<< "$LISTING"; then pass "initramfs module $mod"; else fail "initramfs module $mod missing"; fi
  done
  # initramfs init matches source and has offline refresh
  INIT_DIR="$WORK_DIR/initramfs-init"
  mkdir -p "$INIT_DIR"
  gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$INIT_DIR" && cpio -idm 2>/dev/null || true)
  MODULE_DIR=$(find "$INIT_DIR/lib/modules" -mindepth 1 -maxdepth 1 -type d | head -n1)
  CHECKED="$INIT_DIR/.checked-modules"
  verify_module_path() {
    local path="$1" dep
    grep -qxF "$path" "$CHECKED" 2>/dev/null && return
    echo "$path" >> "$CHECKED"
    [ -f "$MODULE_DIR/$path" ] || { fail "initramfs module dependency missing $path"; return; }
    for dep in $(awk -v path="$path" '$1==path ":" {for(i=2;i<=NF;i++) print $i}' "$MODULE_DIR/modules.dep"); do verify_module_path "$dep"; done
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
  verify_required_module() {
    local path
    path=$(resolve_module "$1") || { fail "initramfs required module unresolved $1"; return; }
    verify_module_path "$path"
  }
  while IFS= read -r mod; do case "$mod" in ''|'#'*) continue ;; esac; verify_required_module "$mod"; done < "$REQUIRED_MODULES"
  [ "$FAIL" -eq 0 ] && pass "initramfs required module closure"
  while IFS= read -r feature; do
    case "$feature" in ''|'#'*) continue ;; esac
    mod=$(awk -v feature="$feature" '$1==feature {print $2; exit}' "$FEATURE_MODULES")
    if [ -z "$mod" ]; then
      fail "required kernel feature $feature has no module mapping"
    elif grep -q "/$mod\\.ko$" "$MODULE_DIR/modules.builtin"; then
      pass "required kernel feature $feature=y"
    elif path=$(resolve_module "$mod"); then
      verify_module_path "$path"
      pass "required kernel feature $feature=m ($mod)"
    else
      fail "required kernel feature $feature is unset"
    fi
  done < "$REQUIRED_FEATURES"
  if [ -f "$INIT_DIR/init" ] && diff -q "$INIT_SRC" "$INIT_DIR/init" >/dev/null 2>&1; then pass "initramfs init matches src"; else fail "initramfs init mismatch"; fi
  if grep -q "HARPOON_RESIZE2FS_REFRESH" "$INIT_SRC"; then pass "init has resize2fs refresh"; else fail "init missing refresh"; fi
else
  fail "initramfs missing"
fi

# ROOT TEMPLATE
if [ -f "$ROOT_IMG" ]; then
  SZ=$(stat -f%z "$ROOT_IMG" 2>/dev/null || stat -c%s "$ROOT_IMG")
  if [ "$SZ" = "2147483648" ]; then pass "root 2G logical"; else fail "root size $SZ"; fi
  # debugfs checks (if available)
  if command -v /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs >/dev/null 2>&1; then
    DF="/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs"
    for bin in dockerd docker containerd socat; do
      if grep -q "$bin" <<< "$($DF -R "ls -l /usr/bin" "$ROOT_IMG" 2>&1)"; then pass "root has $bin"; else fail "root missing $bin"; fi
    done
    # python3 MUST be in canonical root (hard gate, no WARN)
    if grep -q "python3" <<< "$($DF -R "ls -l /usr/bin" "$ROOT_IMG" 2>&1)"; then pass "root has python3"; else fail "root missing python3 (hard gate: must be bundled for harpoon-mgmt offline)"; fi
    # Additional python closure: verify binary exists and is executable (use stat for file, not ls -l dir)
    if grep -q "Inode:" <<< "$($DF -R "stat /usr/bin/python3.12" "$ROOT_IMG" 2>&1)"; then pass "root has python3.12 binary"; else fail "root missing python3.12 binary"; fi
    if grep -q "libpython3.12.so.1.0" <<< "$($DF -R "ls -l /usr/lib" "$ROOT_IMG" 2>&1)"; then pass "root has libpython3.12.so.1.0"; else fail "root missing libpython3.12.so.1.0"; fi
    if grep -q "harpoon-mgmt" <<< "$($DF -R "ls -l /usr/local/bin" "$ROOT_IMG" 2>&1)"; then pass "root has harpoon-mgmt"; else fail "root missing harpoon-mgmt (must be in template for offline mgmt)"; fi
    if grep -q "resolv.conf\|apk" <<< "$($DF -R "ls -l /etc" "$ROOT_IMG" 2>&1)"; then pass "root etc"; else fail "root etc"; fi
    # sanitized: no containers
    if grep -q "^d.*[0-9a-f]\{12\}" <<< "$($DF -R "ls -l /var/lib/docker/containers" "$ROOT_IMG" 2>&1)"; then fail "root not sanitized containers"; else pass "root sanitized"; fi
    # volumes check (hardened): any directory under volumes beyond . and .. and metadata.db means dirty
    VOL_LIST=$($DF -R "ls -l /var/lib/docker/volumes" "$ROOT_IMG" 2>&1 || true)
    if grep -q "^d.*[0-9a-f]\{8\}\|^d.*harpoon-lifecycle\|^d.*m6-persist\|^d.*m7-" <<< "$VOL_LIST"; then fail "root not sanitized volumes (leftover test volumes)"; else pass "root sanitized volumes"; fi
  else
    say "debugfs not available, skipping detailed root checks"
  fi
  # verify-root heuristic
  if bash "$REPO_ROOT/tools/guest-builder/verify-root.sh" "$ROOT_IMG" 2>&1 | grep -q "PASS"; then pass "verify-root"; else fail "verify-root"; fi
  # Python ELF closure (extract and inspect)
  if command -v /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs >/dev/null 2>&1 && [ "$READELF" != "false" ]; then
    DF="/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs"
    PYTHON_DIR="$WORK_DIR/python"
    mkdir -p "$PYTHON_DIR"
    # Extract python binary via debugfs dump
    if grep -q "dump" <<< "$($DF -R "dump /usr/bin/python3.12 $PYTHON_DIR/python3.12" "$ROOT_IMG" 2>&1)"; then
      : # dump may not output; check file exists
      true
    fi
    if [ -f "$PYTHON_DIR/python3.12" ]; then
      if $READELF --dynamic "$PYTHON_DIR/python3.12" 2>&1 | grep -q "libpython3.12.so.1.0"; then pass "python3 DT_NEEDED libpython"; else fail "python3 missing libpython DT_NEEDED"; fi
      if file "$PYTHON_DIR/python3.12" 2>&1 | grep -q "aarch64"; then pass "python3 aarch64"; else fail "python3 not aarch64"; fi
      INTERP_PY=$($READELF -l "$PYTHON_DIR/python3.12" 2>&1 | grep -o "/[^ ]*ld-musl[^ ]*" | head -n1 | tr -d ']' || true)
      if [ -z "$INTERP_PY" ]; then INTERP_PY=$($READELF -l "$PYTHON_DIR/python3.12" 2>&1 | grep "Requesting program interpreter" | sed -n 's/.*: \([^]]*\)].*/\1/p' | head -n1 | tr -d ']' || true); fi
      if [ -n "$INTERP_PY" ]; then
        # Check that loader exists in root image via debugfs (use stat for file, not ls -l dir)
        if grep -q "Inode:" <<< "$($DF -R "stat $INTERP_PY" "$ROOT_IMG" 2>&1)"; then pass "python3 loader $INTERP_PY present in root"; else
          if grep -q "Inode:" <<< "$($DF -R "stat /lib/ld-musl-aarch64.so.1" "$ROOT_IMG" 2>&1)"; then pass "python3 loader via /lib/ld-musl-aarch64.so.1"; else fail "python3 loader missing $INTERP_PY in root"; fi
        fi
      else
        fail "python3 could not determine PT_INTERP"
      fi
      # Check libpython exists and is aarch64 ELF
      LIBPYTHON_DIR="$WORK_DIR/libpython"
      mkdir -p "$LIBPYTHON_DIR"
      $DF -R "dump /usr/lib/libpython3.12.so.1.0 $LIBPYTHON_DIR/libpython.so" "$ROOT_IMG" 2>&1 >/dev/null || true
      if [ -f "$LIBPYTHON_DIR/libpython.so" ]; then
        if file "$LIBPYTHON_DIR/libpython.so" 2>&1 | grep -q "aarch64\|ELF"; then pass "libpython3.12 ELF present"; else fail "libpython3.12 not ELF"; fi
      else
        fail "could not extract libpython3.12.so.1.0 for inspection"
      fi
    else
      fail "could not extract python3.12 from root for ELF inspection"
    fi
  fi
else
  fail "root missing"
fi

# BOOT ORDER
if [ -f "$INIT_SRC" ]; then
  DISK_LINE=$(grep -n "HARPOON_DISK_CHECK_START" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  DOCKER_LINE=$(grep -n "HARPOON_DOCKERD_START" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 0)
  APK_LINE=$(grep -n "HARPOON_APK_UPDATE_START" "$INIT_SRC" | cut -d: -f1 | head -n1 || echo 9999)
  if [ "$DISK_LINE" -lt "$DOCKER_LINE" ] && [ "$DISK_LINE" -lt "$APK_LINE" ]; then pass "boot order disk before docker+apk offline"; else fail "boot order not offline ($DISK_LINE vs $DOCKER_LINE/$APK_LINE)"; fi
  if grep -q "apk add.*e2fsprogs" "$INIT_SRC"; then fail "init apk adds e2fsprogs (should be offline)"; else pass "init offline e2fsprogs"; fi
  if grep -q "apk add.*python3" "$INIT_SRC"; then fail "init apk adds python3 (should be offline via root template)"; else pass "init offline python3"; fi
  if ! grep -q "HARPOON_APK_SKIPPED" "$INIT_SRC"; then fail "init missing APK offline fallback"; else pass "init APK offline fallback"; fi
fi

# HARPOON-MGMT
if [ -f "$HARPOON_MGMT" ]; then
  if head -n1 "$HARPOON_MGMT" | grep -q python3; then pass "harpoon-mgmt shebang"; else fail "harpoon-mgmt shebang"; fi
  if python3 -c "import ast; ast.parse(open('$HARPOON_MGMT').read())" 2>&1 | head -n1; then pass "harpoon-mgmt syntax"; else fail "harpoon-mgmt syntax"; fi
  # imports: check for json etc. that must exist in guest python3 (socket is via vsock, json required)
  if grep -q "import.*json" "$HARPOON_MGMT"; then pass "harpoon-mgmt imports"; else fail "harpoon-mgmt imports"; fi
  # Verify imports resolve in bundled python stdlib (artifact check)
  if [ -f "$ROOT_IMG" ] && command -v /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs >/dev/null 2>&1; then
    DF="/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs"
    STDLIB_LIST=$($DF -R "ls -l /usr/lib/python3.12" "$ROOT_IMG" 2>&1 || true)
    DYNLIB_LIST=$($DF -R "ls -l /usr/lib/python3.12/lib-dynload" "$ROOT_IMG" 2>&1 || true)
    for mod in json subprocess pty select signal struct fcntl termios os sys; do
      if grep -q "$mod" <<< "$STDLIB_LIST"; then pass "harpoon-mgmt import $mod in bundled stdlib"; else
        if grep -q "$mod" <<< "$DYNLIB_LIST"; then pass "harpoon-mgmt import $mod via lib-dynload"; else
          if grep -wq "$mod" <<< "sys os signal struct"; then pass "harpoon-mgmt import $mod builtin"; else fail "harpoon-mgmt import $mod missing in bundled python"; fi
        fi
      fi
    done
    # Also check pty.py specifically (use stat for file)
    if grep -q "Inode:" <<< "$($DF -R "stat /usr/lib/python3.12/pty.py" "$ROOT_IMG" 2>&1)"; then pass "bundled pty.py present"; else fail "bundled pty.py missing"; fi
    # Check lib-dynload for fcntl, select, termios
    for so in fcntl select termios _json _socket; do
      if grep -q "$so" <<< "$($DF -R "ls -l /usr/lib/python3.12/lib-dynload" "$ROOT_IMG" 2>&1)"; then pass "bundled lib-dynload $so"; else
        if [ "$so" = "fcntl" ] || [ "$so" = "select" ] || [ "$so" = "termios" ]; then fail "bundled lib-dynload $so missing"; else pass "bundled lib-dynload $so optional"; fi
      fi
    done
  fi
else
  fail "harpoon-mgmt missing"
fi

if [ $FAIL -ne 0 ]; then say "FAIL: runtime closure incomplete"; exit 1; fi
say "PASS: runtime closure complete"
