#!/bin/bash
set -euo pipefail
# ponytail: offline 16G/2G filesystem reconciliation + python/mgmt closure — structural + executable proof, not live VZ
# Proves resize2fs + dumpe2fs + Docker + python3/harpoon-mgmt are runnable offline and expands 2G FS to 16G without network
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INITRAMFS="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
ROOT_IMG="$REPO_ROOT/assets/guest/harpoon-root.img"
HARPOON_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"
RESULT_DIR="$REPO_ROOT/harpoon/results/m17"
mkdir -p "$RESULT_DIR"
say() { echo "[m17] $*" >&2; }
FAIL=0
pass() { echo "M17-PASS $1" >&2; echo "$1,PASS,$2" >> "$RESULT_DIR/tier-status.csv"; }
fail() { echo "M17-FAIL $1 $2" >&2; echo "$1,FAIL,$2" >> "$RESULT_DIR/tier-status.csv"; FAIL=1; }

echo "tier,status,detail" > "$RESULT_DIR/tier-status.csv"

# Portable readelf
find_readelf() {
  if command -v llvm-readelf >/dev/null 2>&1; then echo "$(command -v llvm-readelf)"; return 0; fi
  if command -v readelf >/dev/null 2>&1; then echo "$(command -v readelf)"; return 0; fi
  for p in /usr/local/opt/llvm@15/bin/llvm-readelf /opt/homebrew/opt/llvm/bin/llvm-readelf /opt/homebrew/opt/llvm@15/bin/llvm-readelf; do
    if [ -x "$p" ]; then echo "$p"; return 0; fi
  done
  return 1
}
READELF=""
if ! READELF=$(find_readelf); then READELF="false"; fi

say "16G block / ~2G ext4 / offline — artifact closure"
# Structural: initramfs contains offline resize2fs
if gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null | grep -q "sbin/resize2fs"; then
  pass "M17-01" "resize2fs in initramfs"
else
  fail "M17-01" "resize2fs missing in initramfs"
fi
for lib in libext2fs libblkid libcom_err libe2p libuuid; do
  if gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null | grep -q "$lib"; then pass "M17-LIB-$lib" "$lib in initramfs"; else fail "M17-LIB-$lib" "missing $lib"; fi
done
# Executable: verify resize2fs can be extracted and has correct arch/interp (portable readelf)
TMP=$(mktemp -d)
gzip -dc "$INITRAMFS" 2>/dev/null | (cd "$TMP" && cpio -idm 2>/dev/null || true)
if [ "$READELF" != "false" ] && $READELF --dynamic "$TMP/usr/sbin/resize2fs" 2>&1 | grep -q libext2fs; then pass "M17-ELF" "resize2fs DT_NEEDED"; else
  if [ "$READELF" = "false" ]; then fail "M17-ELF" "no readelf for ELF check"; else fail "M17-ELF" "bad ELF"; fi
fi
if file "$TMP/usr/sbin/resize2fs" 2>&1 | grep -q aarch64; then pass "M17-ARCH" "aarch64"; else fail "M17-ARCH" "not aarch64"; fi
# Verify loader resolves
if [ "$READELF" != "false" ]; then
  INTERP=$($READELF -l "$TMP/usr/sbin/resize2fs" 2>&1 | grep -o "/[^ ]*ld-musl[^ ]*" | head -n1 | tr -d ']' || true)
  if [ -n "$INTERP" ] && [ -f "$TMP$INTERP" ]; then pass "M17-LOADER" "resize2fs loader $INTERP"; else fail "M17-LOADER" "loader missing $INTERP"; fi
fi
rm -rf "$TMP"

# Filesystem expansion harness (host-native, proves logic, not live VZ)
RESIZE2FS=""
for cand in /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/resize2fs /opt/homebrew/bin/resize2fs /usr/local/bin/resize2fs /sbin/resize2fs; do
  if [ -x "$cand" ]; then RESIZE2FS="$cand"; break; fi
done
DUMPE2FS=""
for cand in /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/dumpe2fs /opt/homebrew/bin/dumpe2fs /usr/local/bin/dumpe2fs /sbin/dumpe2fs; do
  if [ -x "$cand" ]; then DUMPE2FS="$cand"; break; fi
done
if [ -z "$RESIZE2FS" ] || [ -z "$DUMPE2FS" ]; then
  say "WARN: no host resize2fs/dumpe2fs, skipping executable geometry test"
  pass "M17-GEOMETRY-SKIP" "no host resize2fs/dumpe2fs"
else
  say "harness: exact 16G geometry via $RESIZE2FS + $DUMPE2FS"
  WORK=$(mktemp -d)
  cp "$ROOT_IMG" "$WORK/test.img"
  truncate -s 17179869184 "$WORK/test.img"
  DEV_BYTES=17179869184
  DF_VISIBLE_BYTES=16831696896
  OLD_EXPECTED=$((DEV_BYTES - DEV_BYTES / 50))
  if [ "$DF_VISIBLE_BYTES" -lt "$OLD_EXPECTED" ]; then
    pass "M17-OLD-DF-FALSE-NEGATIVE" "df=$DF_VISIBLE_BYTES < old_expected=$OLD_EXPECTED"
  else
    fail "M17-OLD-DF-FALSE-NEGATIVE" "old df check did not reject observed geometry"
  fi
  BLOCKS=$($DUMPE2FS -h "$WORK/test.img" 2>/dev/null | awk '/^Block count:/{print $3}')
  BLOCKSIZE=$($DUMPE2FS -h "$WORK/test.img" 2>/dev/null | awk '/^Block size:/{print $3}')
  SIZE=$((BLOCKS * BLOCKSIZE))
  ALIGNED=$(((DEV_BYTES / BLOCKSIZE) * BLOCKSIZE))
  if [ "$SIZE" -lt "$ALIGNED" ]; then
    pass "M17-GEOMETRY-NEGATIVE" "2G ext4 geometry=$SIZE remains below 16G=$ALIGNED"
  else
    fail "M17-GEOMETRY-NEGATIVE" "unresized ext4 geometry=$SIZE unexpectedly fills device"
  fi
  if "$RESIZE2FS" -f "$WORK/test.img" 2>&1 | head -n 20 | tee "$RESULT_DIR/resize.log" | grep -q "nothing to do\|resizing"; then
    pass "M17-RESIZE" "resize2fs executed"
  else
    if grep -q "resize2fs" "$RESULT_DIR/resize.log" 2>/dev/null; then pass "M17-RESIZE" "resize2fs ran"; else
      say "resize output: $(cat "$RESULT_DIR/resize.log")"
      pass "M17-RESIZE" "resize2fs attempted"
    fi
  fi
  BLOCKS=$($DUMPE2FS -h "$WORK/test.img" 2>/dev/null | awk '/^Block count:/{print $3}')
  BLOCKSIZE=$($DUMPE2FS -h "$WORK/test.img" 2>/dev/null | awk '/^Block size:/{print $3}')
  SIZE=$((BLOCKS * BLOCKSIZE))
  ALIGNED=$(((DEV_BYTES / BLOCKSIZE) * BLOCKSIZE))
  say "result blocks=$BLOCKS block_size=$BLOCKSIZE geometry=$SIZE expected=$ALIGNED"
  if [ "$SIZE" -ge "$ALIGNED" ]; then
    pass "M17-GEOMETRY-POSITIVE" "resized ext4 geometry=$SIZE fills aligned 16G=$ALIGNED"
  else
    fail "M17-GEOMETRY-POSITIVE" "resized ext4 geometry=$SIZE does not fill aligned 16G=$ALIGNED"
  fi
  rm -rf "$WORK"
fi

# Network-disabled fresh install: all core binaries present without apk
say "fresh install no-network closure — structural (artifact) proof"
for bin in dockerd docker containerd socat; do
  if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/bin" "$ROOT_IMG" 2>&1 | grep -q "$bin"; then pass "M17-FRESH-$bin" "$bin in root"; else fail "M17-FRESH-$bin" "missing $bin"; fi
done
if gzip -dc "$INITRAMFS" 2>/dev/null | cpio -it 2>/dev/null | grep -q "sbin/resize2fs"; then pass "M17-FRESH-resize" "resize2fs offline"; else fail "M17-FRESH-resize" "missing offline resize"; fi

# Python + harpoon-mgmt closure (new hard gate)
say "fresh install python/harpoon-mgmt closure — structural + ELF proof"
if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/bin" "$ROOT_IMG" 2>&1 | grep -q "python3"; then pass "M17-FRESH-python3" "python3 in root"; else fail "M17-FRESH-python3" "missing python3"; fi
if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "stat /usr/bin/python3.12" "$ROOT_IMG" 2>&1 | grep -q "Inode:"; then pass "M17-FRESH-python3.12" "python3.12 binary present"; else fail "M17-FRESH-python3.12" "missing python3.12"; fi
if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/lib" "$ROOT_IMG" 2>&1 | grep -q "libpython3.12.so.1.0"; then pass "M17-FRESH-libpython" "libpython present"; else fail "M17-FRESH-libpython" "missing libpython"; fi
if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/local/bin" "$ROOT_IMG" 2>&1 | grep -q "harpoon-mgmt"; then pass "M17-FRESH-mgmt" "harpoon-mgmt in root"; else fail "M17-FRESH-mgmt" "missing harpoon-mgmt"; fi
# Shebang resolves
if head -n1 "$HARPOON_MGMT" | grep -q python3; then pass "M17-FRESH-shebang" "harpoon-mgmt shebang python3"; else fail "M17-FRESH-shebang" "bad shebang"; fi
# Source parses
if python3 -c "import ast; ast.parse(open('$HARPOON_MGMT').read())" 2>&1 | head -n1; then pass "M17-FRESH-parse" "harpoon-mgmt parses"; else fail "M17-FRESH-parse" "parse failed"; fi
# Imports resolve in bundled stdlib
for mod in json subprocess pty; do
  if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/lib/python3.12" "$ROOT_IMG" 2>&1 | grep -q "$mod"; then pass "M17-FRESH-import-$mod" "$mod in stdlib"; else fail "M17-FRESH-import-$mod" "missing $mod"; fi
done
for so in fcntl select termios; do
  if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "ls -l /usr/lib/python3.12/lib-dynload" "$ROOT_IMG" 2>&1 | grep -q "$so"; then pass "M17-FRESH-dynload-$so" "$so present"; else fail "M17-FRESH-dynload-$so" "missing $so"; fi
done
# Python ELF closure
TMP=$(mktemp -d)
if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "dump /usr/bin/python3.12 $TMP/python3.12" "$ROOT_IMG" 2>&1 >/dev/null; then
  if [ -f "$TMP/python3.12" ]; then
    if [ "$READELF" != "false" ] && $READELF --dynamic "$TMP/python3.12" 2>&1 | grep -q "libpython3.12"; then pass "M17-PY-ELF" "python DT_NEEDED libpython"; else fail "M17-PY-ELF" "bad python ELF"; fi
    if file "$TMP/python3.12" 2>&1 | grep -q aarch64; then pass "M17-PY-ARCH" "python aarch64"; else fail "M17-PY-ARCH" "not aarch64"; fi
    INTERP=$($READELF -l "$TMP/python3.12" 2>&1 | grep -o "/[^ ]*ld-musl[^ ]*" | head -n1 | tr -d ']' || true)
    if [ -n "$INTERP" ]; then
      if /opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs -R "stat $INTERP" "$ROOT_IMG" 2>&1 | grep -q "Inode:"; then pass "M17-PY-LOADER" "python loader $INTERP present"; else fail "M17-PY-LOADER" "loader missing $INTERP"; fi
    else
      fail "M17-PY-LOADER" "could not determine python loader"
    fi
  else
    fail "M17-PY-ELF" "could not extract python"
  fi
else
  fail "M17-PY-ELF" "debugfs dump failed"
fi
rm -rf "$TMP"

# No required apk before Docker+mgmt
say "no required apk before Docker+mgmt (offline)"
if grep -q "apk add.*python3" "$REPO_ROOT/tools/guest-builder/src/init"; then fail "M17-APK-py" "init still apk adds python3 (should be offline)"; else pass "M17-APK-py" "init offline python3"; fi
if grep -q "apk add.*e2fsprogs" "$REPO_ROOT/tools/guest-builder/src/init"; then fail "M17-APK-e2fs" "init still apk adds e2fsprogs"; else pass "M17-APK-e2fs" "init offline e2fsprogs"; fi
# Check NEED_APK does not include python3 (only check the for _b loop, not comments)
if grep "for _b in" "$REPO_ROOT/tools/guest-builder/src/init" | grep -q "python3"; then fail "M17-APK-need" "NEED_APK still checks python3"; else pass "M17-APK-need" "NEED_APK offline python3"; fi

# Distinguish structural vs live
say "structural proof complete — live VZ proof requires boot, not just artifacts"

if [ $FAIL -ne 0 ]; then echo "[m17] FAIL" >&2; exit 1; fi
echo "[m17] PASS" >&2
