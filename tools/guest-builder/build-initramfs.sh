#!/bin/bash
set -euo pipefail
# ponytail: build canonical harpoon-initramfs.cpio.gz — deterministic, no bootstrap cycle
# Precedence:
#   1. explicit FETCH env (HARPOON_INITRAMFS_URL/SHA256) — versioned artifact cache
#   2. local canonical if its input manifest matches
#   3. deterministic REBUILD via Docker Linux (Alpine 3.22)
#   4. BOOTSTRAP only if HARPOON_ALLOW_BOOTSTRAP=1 (development-only, not release)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="${HARPOON_INITRAMFS_OUT:-$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz}"
FINGERPRINT="$OUT.inputs"
BOOTSTRAP="$REPO_ROOT/assets/guest/.bootstrap/harpoon-initramfs.cpio.gz"
CACHE_DIR="$REPO_ROOT/assets/guest/.cache"
SRC_INIT="$REPO_ROOT/tools/guest-builder/src/init"
SRC_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"
SRC_MGMT_WRAPPER="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt-wrapper"
ROOT_IMG="$REPO_ROOT/assets/guest/harpoon-root.img"
mkdir -p "$REPO_ROOT/assets/guest" "$CACHE_DIR"

# Pinned production inputs — mismatch FAILs
EXPECTED_VMLINUZ_SHA="f270bfa4324e37f0a28662909b0450c802c8279143f353cbc7fe250cdfb733a8"
EXPECTED_INITRAMFS_VIRT_SHA="508de7f561b94aac0b569611574502e4528eb21230318badac9626b7f1791bf4"
EXPECTED_MODLOOP_SHA="65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810"
EXPECTED_MINIROOTFS_SHA="188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c"
EXPECTED_KERNEL="6.12.94-0-virt"

input_manifest() {
  local file rel
  printf 'harpoon-initramfs-inputs-v1\n'
  printf 'pin %s %s\n' kernel "$EXPECTED_KERNEL"
  printf 'pin %s %s\n' vmlinuz "$EXPECTED_VMLINUZ_SHA"
  printf 'pin %s %s\n' initramfs-virt "$EXPECTED_INITRAMFS_VIRT_SHA"
  printf 'pin %s %s\n' modloop "$EXPECTED_MODLOOP_SHA"
  printf 'pin %s %s\n' minirootfs "$EXPECTED_MINIROOTFS_SHA"
  for file in "$SCRIPT_DIR/build-initramfs.sh" "$SRC_INIT" "$SRC_MGMT" "$SRC_MGMT_WRAPPER" "$REPO_ROOT/tools/guest-builder/required-modules.txt" "$REPO_ROOT/tools/guest-builder/required-kernel-features.txt" "$REPO_ROOT/tools/guest-builder/kernel-feature-modules.txt" "$ROOT_IMG"; do
    [ -f "$file" ] || { echo "[build-initramfs] FAIL: missing input $file" >&2; return 1; }
    rel="${file#$REPO_ROOT/}"
    printf 'sha256 %s %s\n' "$(shasum -a 256 "$file" | cut -d' ' -f1)" "$rel"
  done
}

CURRENT_INPUTS=$(mktemp)
trap 'rm -f "$CURRENT_INPUTS"' EXIT
input_manifest > "$CURRENT_INPUTS"

case "${1:-}" in
  --fingerprint) cat "$CURRENT_INPUTS"; exit 0 ;;
  --check)
    if [ -f "$OUT" ] && [ -f "$FINGERPRINT" ] && cmp -s "$CURRENT_INPUTS" "$FINGERPRINT"; then
      echo "[build-initramfs] current: $OUT" >&2
      exit 0
    fi
    echo "[build-initramfs] stale: $OUT" >&2
    exit 1
    ;;
  '') ;;
  *) echo "usage: $0 [--check|--fingerprint]" >&2; exit 2 ;;
esac

write_fingerprint() {
  cp "$CURRENT_INPUTS" "$FINGERPRINT.tmp"
  mv "$FINGERPRINT.tmp" "$FINGERPRINT"
}

# Verify pinned cache inputs if present (mismatch FAIL, not just print)
for _f in "$CACHE_DIR/vmlinuz-virt" "$CACHE_DIR/initramfs-virt" "$CACHE_DIR/modloop-virt" "$CACHE_DIR/alpine-minirootfs-3.22.1-aarch64.tar.gz"; do
  if [ -f "$_f" ]; then
    case "$_f" in
      *vmlinuz-virt) _exp="$EXPECTED_VMLINUZ_SHA" ;;
      *initramfs-virt) _exp="$EXPECTED_INITRAMFS_VIRT_SHA" ;;
      *modloop-virt) _exp="$EXPECTED_MODLOOP_SHA" ;;
      *minirootfs*) _exp="$EXPECTED_MINIROOTFS_SHA" ;;
      *) continue ;;
    esac
    _actual=$(shasum -a 256 "$_f" | cut -d' ' -f1)
    if [ "$_actual" != "$_exp" ]; then echo "[build-initramfs] FAIL: $_f sha mismatch expected $_exp got $_actual" >&2; exit 1; fi
  fi
done

# 1. explicit FETCH env
RELEASE_URL="${HARPOON_INITRAMFS_URL:-}"
RELEASE_SHA="${HARPOON_INITRAMFS_SHA256:-}"
DEFAULT_RELEASE_URL="https://github.com/Harpoon/releases/download/v0.1.1/harpoon-initramfs.cpio.gz"
DEFAULT_SHA="" # TODO: populate after v0.1.1 publish (docs/building.md provenance)
FETCH_URL=""
FETCH_SHA=""
if [ -n "$RELEASE_URL" ]; then
  FETCH_URL="$RELEASE_URL"
  FETCH_SHA="$RELEASE_SHA"
elif [ -n "$DEFAULT_SHA" ]; then
  FETCH_URL="$DEFAULT_RELEASE_URL"
  FETCH_SHA="$DEFAULT_SHA"
fi
if [ -n "$FETCH_URL" ] && [ -n "$FETCH_SHA" ]; then
  echo "[build-initramfs] FETCH MODE: fetching $FETCH_URL" >&2
  TMP_FETCH="$CACHE_DIR/harpoon-initramfs.cpio.gz.tmp"
  curl -L --fail -o "$TMP_FETCH" "$FETCH_URL"
  ACTUAL_SHA=$(shasum -a 256 "$TMP_FETCH" | cut -d' ' -f1)
  if [ "$ACTUAL_SHA" != "$FETCH_SHA" ]; then
    echo "[build-initramfs] FAIL: SHA mismatch expected $FETCH_SHA got $ACTUAL_SHA" >&2
    rm -f "$TMP_FETCH"
    exit 1
  fi
  mv "$TMP_FETCH" "$OUT"
  rm -f "$FINGERPRINT"
  echo "[build-initramfs] fetched and verified $OUT (sha256 $ACTUAL_SHA)" >&2
  ls -lh "$OUT" >&2
  exit 0
fi

# 2. local canonical only when its declared inputs match
if [ -f "$OUT" ] && [ -f "$FINGERPRINT" ] && cmp -s "$CURRENT_INPUTS" "$FINGERPRINT"; then
  echo "[build-initramfs] up to date at $OUT" >&2
  ls -lh "$OUT" >&2
  exit 0
fi
if [ -f "$OUT" ]; then echo "[build-initramfs] STALE: input manifest missing, malformed, or changed; rebuilding" >&2; fi

# 3. deterministic REBUILD via Docker Linux (Alpine 3.22)
# Pinned inputs (Alpine 3.22 aarch64, kernel 6.12.94-0-virt):
#   Kernel: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/vmlinuz-virt (6.12.94-0-virt)
#   Initramfs-virt: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/initramfs-virt
#   Modloop: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/modloop-virt
#   Minirootfs: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/alpine-minirootfs-3.22.1-aarch64.tar.gz
# Packages pinned at build time via apk add with explicit Alpine 3.22 repos (no latest).
# Required modules injected from modloop via modules.dep dependency walk (virtio, vsock, etc).
# SHA-256 for upstream artifacts where practical (recorded after fetch, verified on rebuild):
#   vmlinuz-virt: f270bfa4324e37f0a28662909b0450c802c8279143f353cbc7fe250cdfb733a8 (cached .cache/vmlinuz-virt)
#   initramfs-virt: 508de7f561b94aac0b569611574502e4528eb21230318badac9626b7f1791bf4 (cached)
#   modloop-virt: (fetched, sha printed at build)
#   alpine-minirootfs-3.22.1: (fetched, sha printed)

CALLER_DOCKER_CONTEXT="${DOCKER_CONTEXT:-}"
DOCKER_CONTEXT="${HARPOON_BUILD_DOCKER_CONTEXT:-}"
docker_cmd() {
  if [ -n "$DOCKER_CONTEXT" ]; then
    env -u DOCKER_HOST -u DOCKER_CONTEXT docker --context "$DOCKER_CONTEXT" "$@"
  else
    docker "$@"
  fi
}
if [ -n "$DOCKER_CONTEXT" ]; then
  ENGINE="HARPOON_BUILD_DOCKER_CONTEXT=$DOCKER_CONTEXT"
elif [ -n "${DOCKER_HOST:-}" ]; then
  ENGINE="DOCKER_HOST=$DOCKER_HOST"
elif [ -n "$CALLER_DOCKER_CONTEXT" ]; then
  DOCKER_CONTEXT="$CALLER_DOCKER_CONTEXT"
  ENGINE="DOCKER_CONTEXT=$DOCKER_CONTEXT"
elif docker info >/dev/null 2>&1; then
  ENGINE="default Docker endpoint"
elif docker --context harpoon info >/dev/null 2>&1; then
  DOCKER_CONTEXT=harpoon
  ENGINE="harpoon Docker context"
fi
if [ -n "${ENGINE:-}" ] && docker_cmd info >/dev/null 2>&1; then
  echo "[build-initramfs] build engine: $ENGINE" >&2
  echo "[build-initramfs] REBUILD MODE: deterministic rebuild via Docker (Alpine 3.22)..." >&2
  # Ensure src init/mgmt exist (committed)
  if [ ! -f "$SRC_INIT" ]; then echo "[build-initramfs] FAIL: missing $SRC_INIT (committed init source)" >&2; exit 1; fi
  if [ ! -f "$SRC_MGMT" ]; then echo "[build-initramfs] FAIL: missing $SRC_MGMT" >&2; exit 1; fi
  # Run rebuild inside Alpine container
  docker_cmd run --rm -v "$REPO_ROOT:/repo" -v "$CACHE_DIR:/cache" alpine:3.22 sh -c '
    set -euo pipefail
    apk add --no-cache cpio gzip squashfs-tools curl e2fsprogs e2fsprogs-extra > /dev/null
    BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64"
    CACHE="/cache"
    REPO="/repo"
    OUT="/repo/assets/guest/harpoon-initramfs.cpio.gz"
    # Fetch modloop and minirootfs to cache if missing
    if [ ! -f "$CACHE/modloop-virt" ]; then
      echo "[rebuild] fetching modloop-virt..." >&2
      curl -L --fail -o "$CACHE/modloop-virt" "$BASE/netboot/modloop-virt"
      echo "[rebuild] modloop sha256 $(sha256sum "$CACHE/modloop-virt" | cut -d" " -f1)" >&2
    fi
    if [ "$(sha256sum "$CACHE/modloop-virt" | awk "{print \$1}")" != "65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810" ]; then echo "[rebuild] FAIL modloop sha mismatch expected 65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810" >&2; exit 1; fi
    if [ ! -f "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" ]; then
      echo "[rebuild] fetching alpine-minirootfs-3.22.1..." >&2
      curl -L --fail -o "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" "$BASE/alpine-minirootfs-3.22.1-aarch64.tar.gz"
      echo "[rebuild] minirootfs sha256 $(sha256sum "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" | cut -d" " -f1)" >&2
    fi
    if [ "$(sha256sum "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" | awk "{print \$1}")" != "188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c" ]; then echo "[rebuild] FAIL minirootfs sha mismatch expected 188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c" >&2; exit 1; fi
    # Prepare staging
    STAGING=$(mktemp -d)
    echo "[rebuild] staging at $STAGING" >&2
    tar -xzf "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" -C "$STAGING"
    # Extract modloop squashfs
    MODLOOP_TMP=$(mktemp -d)
    unsquashfs -f -d "$MODLOOP_TMP/modloop" "$CACHE/modloop-virt" > /dev/null
    # Find kernel version dir
    MODULES_DIR="$MODLOOP_TMP/modloop/modules"
    KVER=$(ls "$MODULES_DIR" | head -n1)
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
        [ -n "$alias" ] && path=$(find "$MODULES_DIR/$KVER" -type f -name "$alias.ko" | sed "s#^$MODULES_DIR/$KVER/##" | LC_ALL=C sort | head -n1)
      fi
      [ -n "$path" ] || { echo "[rebuild] FAIL unresolved required module: $name" >&2; return 1; }
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
    feature_module() {
      awk -v feature="$1" "\$1==feature {print \$2; exit}" "$FEATURE_MODULES"
    }
    while IFS= read -r mod; do
      case "$mod" in ""|"#"*) continue ;; esac
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
    ROOT_IMG="/repo/assets/guest/harpoon-root.img"
    [ -f "$ROOT_IMG" ] || { echo "[rebuild] FAIL missing canonical root image" >&2; exit 1; }
    mkdir -p "$STAGING/usr/bin" "$STAGING/usr/lib" "$STAGING/lib" "$STAGING/usr/local/share/harpoon-runtime"
    debugfs -R "dump /usr/bin/python3.12 $STAGING/usr/bin/python3.12" "$ROOT_IMG" >/dev/null
    ln -s python3.12 "$STAGING/usr/bin/python3"
    debugfs -R "dump /usr/lib/libpython3.12.so.1.0 $STAGING/usr/lib/libpython3.12.so.1.0" "$ROOT_IMG" >/dev/null
    debugfs -R "dump /lib/ld-musl-aarch64.so.1 $STAGING/lib/ld-musl-aarch64.so.1" "$ROOT_IMG" >/dev/null
    debugfs -R "rdump /usr/lib/python3.12 $STAGING/usr/lib" "$ROOT_IMG" >/dev/null
    chmod 0755 "$STAGING/usr/bin/python3.12" "$STAGING/usr/lib/libpython3.12.so.1.0" "$STAGING/lib/ld-musl-aarch64.so.1"
    RUNTIME_MANIFEST="$STAGING/usr/local/share/harpoon-runtime/python.manifest"
    {
      find "$STAGING/usr/bin/python3.12" "$STAGING/usr/lib/libpython3.12.so.1.0" "$STAGING/lib/ld-musl-aarch64.so.1" "$STAGING/usr/lib/python3.12" -type f -print | LC_ALL=C sort | while IFS= read -r path; do
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
  '
  if [ -f "$OUT" ]; then
    write_fingerprint
    echo "[build-initramfs] REBUILD SUCCESS at $OUT" >&2
    ls -lh "$OUT" >&2
    exit 0
  else
    echo "[build-initramfs] REBUILD FAILED: $OUT not created" >&2
  fi
fi

# 4. BOOTSTRAP only if explicitly allowed (development-only)
if [ "${HARPOON_ALLOW_BOOTSTRAP:-}" = "1" ] && [ -f "$BOOTSTRAP" ]; then
  echo "[build-initramfs] BOOTSTRAP MODE (development-only, HARPOON_ALLOW_BOOTSTRAP=1): using $BOOTSTRAP" >&2
  cp -p "$BOOTSTRAP" "$OUT"
  rm -f "$FINGERPRINT"
  ls -lh "$OUT" >&2
  if command -v shasum >/dev/null; then echo "[build-initramfs] sha256 $(shasum -a 256 "$OUT" | cut -d' ' -f1)" >&2; fi
  echo "[build-initramfs] done (bootstrap, not for release)" >&2
  exit 0
fi

echo "[build-initramfs] FAIL: cannot obtain $OUT on fresh clone" >&2
echo "[build-initramfs] No FETCH artifact (set HARPOON_INITRAMFS_URL/SHA256) and REBUILD requires Docker (docker info must succeed)" >&2
echo "[build-initramfs] No BOOTSTRAP at $BOOTSTRAP (requires HARPOON_ALLOW_BOOTSTRAP=1)" >&2
echo "[build-initramfs] BLOCKER: fresh clone cannot independently generate harpoon-initramfs.cpio.gz without Docker" >&2
exit 1
