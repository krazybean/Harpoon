
set -euo pipefail
apk add --no-cache cpio gzip squashfs-tools curl e2fsprogs e2fsprogs-extra > /dev/null
. /repo/tools/guest-builder/guest-arch.sh
BASE="$GUEST_ALPINE_BASE"
CACHE="/cache"
REPO="/repo"
ASSET_DIR="$(guest_asset_dir /repo/assets/guest)"
OUT="$ASSET_DIR/harpoon-initramfs.cpio.gz"
MINIROOTFS="$GUEST_MINIROOTFS"
LOADER="$GUEST_MUSL_LOADER"
# Fetch modloop and minirootfs to cache if missing
if [ ! -f "$CACHE/modloop-virt" ]; then
  echo "[rebuild] fetching modloop-virt..." >&2
  curl -L --fail -o "$CACHE/modloop-virt" "$BASE/netboot/modloop-virt"
  echo "[rebuild] modloop sha256 $(sha256sum "$CACHE/modloop-virt" | cut -d" " -f1)" >&2
fi
if [ "$(sha256sum "$CACHE/modloop-virt" | awk "{print \$1}")" != "$EXPECTED_MODLOOP_SHA" ]; then echo "[rebuild] FAIL modloop sha mismatch expected $EXPECTED_MODLOOP_SHA" >&2; exit 1; fi
if [ ! -f "$CACHE/$MINIROOTFS" ]; then
  echo "[rebuild] fetching alpine-minirootfs-3.22.1..." >&2
  curl -L --fail -o "$CACHE/$MINIROOTFS" "$BASE/$MINIROOTFS"
  echo "[rebuild] minirootfs sha256 $(sha256sum "$CACHE/$MINIROOTFS" | cut -d" " -f1)" >&2
fi
if [ "$(sha256sum "$CACHE/$MINIROOTFS" | awk "{print \$1}")" != "$EXPECTED_MINIROOTFS_SHA" ]; then echo "[rebuild] FAIL minirootfs sha mismatch expected $EXPECTED_MINIROOTFS_SHA" >&2; exit 1; fi
# Prepare staging
STAGING=$(mktemp -d)
echo "[rebuild] staging at $STAGING" >&2
tar -xzf "$CACHE/$MINIROOTFS" -C "$STAGING"
# Extract modloop squashfs
MODLOOP_TMP=$(mktemp -d)
unsquashfs -f -d "$MODLOOP_TMP/modloop" "$CACHE/modloop-virt" > /dev/null
# Find the one kernel version dir with module metadata; firmware is a sibling.
MODULES_DIR="$MODLOOP_TMP/modloop/modules"
KVER_CANDIDATES=""
for module_dir in "$MODULES_DIR"/*; do
  [ -f "$module_dir/modules.dep" ] || continue
  KVER_CANDIDATES="${KVER_CANDIDATES}${KVER_CANDIDATES:+
}$(basename "$module_dir")"
done
if [ -z "$KVER_CANDIDATES" ]; then
  echo "[rebuild] FAIL: no kernel module directory with modules.dep in $MODULES_DIR" >&2
  exit 1
fi
if [ "$(printf '%s\n' "$KVER_CANDIDATES" | wc -l)" -ne 1 ]; then
  printf '[rebuild] FAIL: multiple kernel module directories with modules.dep: %s\n' "$KVER_CANDIDATES" >&2
  exit 1
fi
KVER="$KVER_CANDIDATES"
[ "$KVER" = "$GUEST_KERNEL_VERSION" ] || { echo "[rebuild] FAIL: unexpected modloop kernel $KVER" >&2; exit 1; }
echo "[rebuild] kernel $KVER" >&2
# Create modules dir in staging
mkdir -p "$STAGING/lib/modules/$KVER"
# Copy modules.dep and related metadata first
cp -a "$MODULES_DIR/$KVER"/modules.* "$STAGING/lib/modules/$KVER/" 2>/dev/null || true
REQUIRED_MODULES="/repo/tools/guest-builder/required-modules.txt"
REQUIRED_FEATURES="/repo/tools/guest-builder/required-kernel-features.txt"
FEATURE_MODULES="/repo/tools/guest-builder/kernel-feature-modules.txt"
COPIED_MODULES="$STAGING/.harpoon-modules"
resolve_module() {
  local name="$1" path alias
  path=$(find "$MODULES_DIR/$KVER" -type f -name "$name.ko" | sed "s#^$MODULES_DIR/$KVER/##" | LC_ALL=C sort | head -n1)
  if [ -z "$path" ]; then
    alias=$(awk -v name="$name" "\$1==\"alias\" && \$2==name {print \$3; exit}" "$MODULES_DIR/$KVER/modules.alias")
    [ -n "$alias" ] && path=$(find "$MODULES_DIR/$KVER" -type f \( -name "$alias.ko" -o -name "$(printf %s "$alias" | tr _ -).ko" \) | sed "s#^$MODULES_DIR/$KVER/##" | LC_ALL=C sort | head -n1)
  fi
  [ -n "$path" ] || {
    echo "[rebuild] FAIL unresolved required module: $name" >&2
    grep -h "$name" "$MODULES_DIR/$KVER/modules.builtin" "$MODULES_DIR/$KVER/modules.alias" 2>/dev/null | head -n 10 >&2
    find "$MODULES_DIR/$KVER" -name "*$name*" | head -n 10 >&2
    return 1
  }
  echo "$path"
}
copy_module() {
  local path="$1" dep
  grep -qxF "$path" "$COPIED_MODULES" 2>/dev/null && return
  echo "$path" >> "$COPIED_MODULES"
  for dep in $(awk -v path="$path" "\$1==path \":\" {for(i=2;i<=NF;i++) print \$i}" "$MODULES_DIR/$KVER/modules.dep"); do
    [ -f "$MODULES_DIR/$KVER/$dep" ] || { echo "[rebuild] FAIL missing dependency $dep for $path" >&2; exit 1; }
    copy_module "$dep"
  done
  mkdir -p "$(dirname "$STAGING/lib/modules/$KVER/$path")"
  cp -a "$MODULES_DIR/$KVER/$path" "$STAGING/lib/modules/$KVER/$path"
}
# Built-in modules (e.g. crc32c on the x86_64 kernel) need no .ko in the initramfs.
module_builtin() {
  local name candidate
  for candidate in "$1" "${1}_generic" "lib$1" $(awk -v name="$1" '$1=="alias" && $2==name {print $3}' "$MODULES_DIR/$KVER/modules.alias" 2>/dev/null); do
    name=$(printf '%s' "$candidate" | sed 's/[-_]/[-_]/g')
    grep -q "/$name\\.ko$" "$MODULES_DIR/$KVER/modules.builtin" 2>/dev/null && return 0
  done
  [ -f "$MODULES_DIR/$KVER/modules.builtin.modinfo" ] && tr '\000' '\n' < "$MODULES_DIR/$KVER/modules.builtin.modinfo" | grep -q "\\.alias=$1$"
}
feature_module() {
  awk -v feature="$1" "\$1==feature {print \$2; exit}" "$FEATURE_MODULES"
}
while IFS= read -r mod; do
  case "$mod" in ""|"#"*) continue ;; esac
  if module_builtin "$mod"; then echo "[rebuild] required module $mod=y" >&2; continue; fi
  path=$(resolve_module "$mod") || exit 1
  copy_module "$path"
done < "$REQUIRED_MODULES"
while IFS= read -r feature; do
  case "$feature" in ""|"#"*) continue ;; esac
  mod=$(feature_module "$feature")
  [ -n "$mod" ] || { echo "[rebuild] FAIL required kernel feature $feature has no module mapping" >&2; exit 1; }
  if grep -q "/$mod\\.ko$" "$MODULES_DIR/$KVER/modules.builtin"; then
    echo "[rebuild] kernel feature $feature=y" >&2
  else
    path=$(resolve_module "$mod") || { echo "[rebuild] FAIL required kernel feature $feature is unset" >&2; exit 1; }
    echo "[rebuild] kernel feature $feature=m ($mod)" >&2
    copy_module "$path"
  fi
done < "$REQUIRED_FEATURES"
rm -f "$COPIED_MODULES"
# Record the exact bundled module closure, including modprobe metadata.
mkdir -p "$STAGING/usr/local/share/harpoon-runtime"
MODULE_MANIFEST="$STAGING/usr/local/share/harpoon-runtime/modules.manifest"
{
  find "$STAGING/lib/modules/$KVER" -type f -print | LC_ALL=C sort | while IFS= read -r path; do
    printf "F %s %s %s\\n" "$(sha256sum "$path" | cut -d" " -f1)" "$(stat -c %a "$path")" "${path#$STAGING/}"
  done
  find "$STAGING/lib/modules/$KVER" -type l -print | LC_ALL=C sort | while IFS= read -r path; do
    printf "L %s %s\\n" "$(readlink "$path")" "${path#$STAGING/}"
  done
} > "$MODULE_MANIFEST"
[ "$(grep -c "^F " "$MODULE_MANIFEST")" -gt 0 ] || { echo "[rebuild] FAIL empty module manifest" >&2; exit 1; }
# Copy harpoon init and management handoff
cp -a "/repo/tools/guest-builder/src/init" "$STAGING/init"
chmod +x "$STAGING/init"
mkdir -p "$STAGING/usr/local/bin"
cp -a "/repo/tools/guest-builder/src/harpoon-mgmt" "$STAGING/usr/local/bin/harpoon-mgmt"
chmod +x "$STAGING/usr/local/bin/harpoon-mgmt"
cp -a "/repo/tools/guest-builder/src/harpoon-mgmt-wrapper" "$STAGING/usr/local/bin/harpoon-mgmt-wrapper"
chmod +x "$STAGING/usr/local/bin/harpoon-mgmt-wrapper"
# Carry the offline Python closure from the canonical persistent root.
ROOT_IMG="$ASSET_DIR/harpoon-root.img"
[ -f "$ROOT_IMG" ] || { echo "[rebuild] FAIL missing canonical root image" >&2; exit 1; }
mkdir -p "$STAGING/usr/bin" "$STAGING/usr/lib" "$STAGING/lib" "$STAGING/usr/local/share/harpoon-runtime"
debugfs_extract_file() {
  local source="$1" destination="$2" output status
  output=$(mktemp)
  if debugfs -R "dump $source $destination" "$ROOT_IMG" >"$output" 2>&1; then
    :
  else
    status=$?
    echo "[rebuild] FAIL debugfs extraction failed (operation: dump $source $destination, status: $status)" >&2
    cat "$output" >&2
    rm -f "$output"
    exit 1
  fi
  if grep -Eiq "filesystem not open|block bitmap checksum|bad magic|cannot open|(^|[[:space:]])error(:|[[:space:]]|$)" "$output"; then
    echo "[rebuild] FAIL debugfs could not read canonical root image (operation: dump $source $destination)" >&2
    cat "$output" >&2
    rm -f "$output"
    exit 1
  fi
  rm -f "$output"
  [ -e "$destination" ] || { echo "[rebuild] FAIL debugfs produced no file for dump $source $destination" >&2; exit 1; }
}
debugfs_extract_tree() {
  local source="$1" destination="$2" expected="$3" output status
  output=$(mktemp)
  if debugfs -R "rdump $source $destination" "$ROOT_IMG" >"$output" 2>&1; then
    :
  else
    status=$?
    echo "[rebuild] FAIL debugfs extraction failed (operation: rdump $source $destination, status: $status)" >&2
    cat "$output" >&2
    rm -f "$output"
    exit 1
  fi
  if grep -Eiq "filesystem not open|block bitmap checksum|bad magic|cannot open|(^|[[:space:]])error(:|[[:space:]]|$)" "$output"; then
    echo "[rebuild] FAIL debugfs could not read canonical root image (operation: rdump $source $destination)" >&2
    cat "$output" >&2
    rm -f "$output"
    exit 1
  fi
  rm -f "$output"
  [ -d "$expected" ] && find "$expected" -type f -print -quit | grep -q . || {
    echo "[rebuild] FAIL debugfs produced no files for rdump $source $destination" >&2
    exit 1
  }
}
debugfs_extract_file /usr/bin/python3.12 "$STAGING/usr/bin/python3.12"
ln -s python3.12 "$STAGING/usr/bin/python3"
debugfs_extract_file /usr/lib/libpython3.12.so.1.0 "$STAGING/usr/lib/libpython3.12.so.1.0"
debugfs_extract_file /lib/$LOADER "$STAGING/lib/$LOADER"
debugfs_extract_tree /usr/lib/python3.12 "$STAGING/usr/lib" "$STAGING/usr/lib/python3.12"
chmod 0755 "$STAGING/usr/bin/python3.12" "$STAGING/usr/lib/libpython3.12.so.1.0" "$STAGING/lib/$LOADER"
RUNTIME_MANIFEST="$STAGING/usr/local/share/harpoon-runtime/python.manifest"
{
  find "$STAGING/usr/bin/python3.12" "$STAGING/usr/lib/libpython3.12.so.1.0" "$STAGING/lib/$LOADER" "$STAGING/usr/lib/python3.12" -type f -print | LC_ALL=C sort | while IFS= read -r path; do
    printf "F %s %s %s\\n" "$(sha256sum "$path" | cut -d" " -f1)" "$(stat -c %a "$path")" "${path#$STAGING/}"
  done
  find "$STAGING/usr/bin/python3" "$STAGING/usr/lib/python3.12" -type l -print | LC_ALL=C sort | while IFS= read -r path; do
    printf "L %s %s\\n" "$(readlink "$path")" "${path#$STAGING/}"
  done
} > "$RUNTIME_MANIFEST"
[ "$(grep -c "^F " "$RUNTIME_MANIFEST")" -gt 0 ] || { echo "[rebuild] FAIL empty Python manifest" >&2; exit 1; }
# Offline resize2fs for filesystem reconciliation (no network at boot)
echo "[rebuild] adding offline resize2fs..." >&2
apk add --no-cache e2fsprogs e2fsprogs-extra e2fsprogs-libs libblkid libuuid libcom_err > /dev/null 2>&1
mkdir -p "$STAGING/usr/sbin" "$STAGING/sbin" "$STAGING/usr/lib" "$STAGING/etc"
cp -a /usr/sbin/resize2fs "$STAGING/usr/sbin/" 2>/dev/null || cp -a /sbin/resize2fs "$STAGING/sbin/" 2>/dev/null || true
cp -a /usr/sbin/resize2fs "$STAGING/sbin/resize2fs" 2>/dev/null || true
for _lib in /usr/lib/libext2fs.so.2 /usr/lib/libe2p.so.2 /usr/lib/libcom_err.so.2 /usr/lib/libblkid.so.1 /usr/lib/libuuid.so.1; do
  if [ -f "$_lib" ]; then cp -a "$_lib" "$STAGING/usr/lib/" 2>/dev/null || true; fi
  _real=$(readlink -f "$_lib" 2>/dev/null || echo "")
  if [ -n "$_real" ] && [ -f "$_real" ]; then cp -a "$_real" "$STAGING/usr/lib/" 2>/dev/null || true; fi
done
for _vlib in /usr/lib/libext2fs.so.2.4 /usr/lib/libe2p.so.2.3 /usr/lib/libcom_err.so.2.1 /usr/lib/libblkid.so.1.1.0 /usr/lib/libuuid.so.1.3.0; do
  if [ -f "$_vlib" ]; then cp -a "$_vlib" "$STAGING/usr/lib/" 2>/dev/null || true; fi
done
if [ -f /etc/mke2fs.conf ]; then cp -a /etc/mke2fs.conf "$STAGING/etc/" 2>/dev/null || true; fi
ls -lh "$STAGING/usr/sbin/resize2fs" "$STAGING/sbin/resize2fs" 2>&1 | head -n 20 >&2 || echo "[rebuild] resize2fs not found" >&2
# Ensure busybox/sh exists (from minirootfs)
# Pack initramfs deterministically: sort, fixed timestamps, gzip -n
# Use reproducible cpio: find with sorted, gzip -n (no timestamp)
# Set file mtimes to 0 for determinism
find "$STAGING" -exec touch -h -d "2025-01-01 00:00:00" {} \; 2>/dev/null || true
(cd "$STAGING" && find . -print0 | LC_ALL=C sort -z | cpio --null -o -H newc 2>/dev/null | gzip -n -9 > "$OUT.tmp")
mv "$OUT.tmp" "$OUT"
echo "[rebuild] built $OUT ($(du -h "$OUT" | awk "{print \$1}"))" >&2
if command -v sha256sum >/dev/null; then echo "[rebuild] sha256 $(sha256sum "$OUT" | cut -d" " -f1)" >&2; fi
ls -lh "$OUT" >&2
