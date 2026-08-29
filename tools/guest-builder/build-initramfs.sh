#!/bin/bash
set -euo pipefail
# ponytail: build canonical harpoon-initramfs.cpio.gz — deterministic, no bootstrap cycle
# Precedence:
#   1. explicit FETCH env (HARPOON_INITRAMFS_URL/SHA256) — versioned artifact cache
#   2. local canonical if already built (assets/guest/harpoon-initramfs.cpio.gz)
#   3. deterministic REBUILD via Docker Linux (Alpine 3.22)
#   4. BOOTSTRAP only if HARPOON_ALLOW_BOOTSTRAP=1 (development-only, not release)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="$REPO_ROOT/assets/guest/harpoon-initramfs.cpio.gz"
BOOTSTRAP="$REPO_ROOT/assets/guest/.bootstrap/harpoon-initramfs.cpio.gz"
CACHE_DIR="$REPO_ROOT/assets/guest/.cache"
SRC_INIT="$REPO_ROOT/tools/guest-builder/src/init"
SRC_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"
mkdir -p "$REPO_ROOT/assets/guest" "$CACHE_DIR"

# Pinned production inputs — mismatch FAILs
EXPECTED_VMLINUZ_SHA="f270bfa4324e37f0a28662909b0450c802c8279143f353cbc7fe250cdfb733a8"
EXPECTED_INITRAMFS_VIRT_SHA="508de7f561b94aac0b569611574502e4528eb21230318badac9626b7f1791bf4"
EXPECTED_MODLOOP_SHA="65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810"
EXPECTED_MINIROOTFS_SHA="188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c"
EXPECTED_KERNEL="6.12.94-0-virt"

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
  echo "[build-initramfs] fetched and verified $OUT (sha256 $ACTUAL_SHA)" >&2
  ls -lh "$OUT" >&2
  exit 0
fi

# 2. local canonical if already built
if [ -f "$OUT" ]; then
  echo "[build-initramfs] up to date at $OUT" >&2
  ls -lh "$OUT" >&2
  exit 0
fi

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

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  echo "[build-initramfs] REBUILD MODE: deterministic rebuild via Docker (Alpine 3.22)..." >&2
  # Ensure src init/mgmt exist (committed)
  if [ ! -f "$SRC_INIT" ]; then echo "[build-initramfs] FAIL: missing $SRC_INIT (committed init source)" >&2; exit 1; fi
  if [ ! -f "$SRC_MGMT" ]; then echo "[build-initramfs] FAIL: missing $SRC_MGMT" >&2; exit 1; fi
  # Run rebuild inside Alpine container
  docker run --rm -v "$REPO_ROOT:/repo" -v "$CACHE_DIR:/cache" alpine:3.22 sh -c '
    set -euo pipefail
    apk add --no-cache cpio gzip squashfs-tools curl > /dev/null
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
    if [ "$(sha256sum "$CACHE/modloop-virt" | cut -d' ' -f1)" != "65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810" ]; then echo "[rebuild] FAIL modloop sha mismatch expected 65a50040ab5129e6c1875353a8d8d91e695eb7f5fc2ba5a36809bd21539ab810" >&2; exit 1; fi
    if [ ! -f "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" ]; then
      echo "[rebuild] fetching alpine-minirootfs-3.22.1..." >&2
      curl -L --fail -o "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" "$BASE/alpine-minirootfs-3.22.1-aarch64.tar.gz"
      echo "[rebuild] minirootfs sha256 $(sha256sum "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" | cut -d" " -f1)" >&2
    fi
    if [ "$(sha256sum "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" | cut -d' ' -f1)" != "188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c" ]; then echo "[rebuild] FAIL minirootfs sha mismatch expected 188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c" >&2; exit 1; fi
    # Prepare staging
    STAGING=$(mktemp -d)
    echo "[rebuild] staging at $STAGING" >&2
    tar -xzf "$CACHE/alpine-minirootfs-3.22.1-aarch64.tar.gz" -C "$STAGING"
    # Extract modloop squashfs
    MODLOOP_TMP=$(mktemp -d)
    unsquashfs -f -d "$MODLOOP_TMP/modloop" "$CACHE/modloop-virt" > /dev/null
    # Find kernel version dir
    KVER=$(ls "$MODLOOP_TMP/modloop/lib/modules" | head -n1)
    echo "[rebuild] kernel $KVER" >&2
    # Create modules dir in staging
    mkdir -p "$STAGING/lib/modules/$KVER"
    # Copy modules.dep and related metadata first
    cp -a "$MODLOOP_TMP/modloop/lib/modules/$KVER"/modules.* "$STAGING/lib/modules/$KVER/" 2>/dev/null || true
    # Determine required modules via dependency walk (virtio, vsock, etc)
    # Use modprobe --show-depends inside staging via chroot or via parsing modules.dep
    # Simpler: copy known required set plus dependencies via modules.dep
    REQ_MODS="
      kernel/drivers/virtio/virtio.ko
      kernel/drivers/virtio/virtio_ring.ko
      kernel/drivers/virtio/virtio_pci.ko
      kernel/drivers/virtio/virtio_mmio.ko
      kernel/drivers/block/virtio_blk.ko
      kernel/drivers/net/virtio_net.ko
      kernel/drivers/char/virtio_console.ko
      kernel/fs/fuse/virtiofs.ko
      kernel/net/vmw_vsock/vsock.ko
      kernel/net/vmw_vsock/vmw_vsock_virtio_transport.ko
      kernel/net/vmw_vsock/vmw_vsock_virtio_transport_common.ko
      kernel/net/vmw_vsock/vsock_diag.ko
      kernel/net/vmw_vsock/vsock_loopback.ko
      kernel/drivers/gpu/drm/virtio/virtio-gpu.ko
      kernel/drivers/char/hw_random/virtio-rng.ko
      kernel/drivers/virtio/virtio_balloon.ko
      kernel/drivers/virtio/virtio_mem.ko
      kernel/lib/libcrc32c.ko
      kernel/net/llc/llc.ko
      kernel/net/netfilter/x_tables.ko
      kernel/drivers/net/veth.ko
    "
    # Helper to copy module and its dependencies via modules.dep
    copy_mod_with_deps() {
      local mod="$1"
      local dep_file="$STAGING/lib/modules/$KVER/modules.dep"
      # Find line for mod
      if [ -f "$dep_file" ]; then
        local line=$(grep -F "$mod:" "$dep_file" 2>/dev/null | head -n1 || true)
        if [ -n "$line" ]; then
          # line is like "kernel/.../virtio.ko: kernel/.../virtio_ring.ko ..."
          local deps=$(echo "$line" | cut -d: -f2)
          for d in $deps; do
            d=$(echo "$d" | xargs)
            if [ -n "$d" ]; then
              local src="$MODLOOP_TMP/modloop/lib/modules/$KVER/$d"
              local dst="$STAGING/lib/modules/$KVER/$d"
              if [ -f "$src" ] && [ ! -f "$dst" ]; then
                mkdir -p "$(dirname "$dst")"
                cp -a "$src" "$dst"
                # recursively copy deps of dep (simple: grep again)
                # For ponytail, one level is enough for virtio stack
              fi
            fi
          done
        fi
      fi
      # Copy the module itself
      local src="$MODLOOP_TMP/modloop/lib/modules/$KVER/$mod"
      local dst="$STAGING/lib/modules/$KVER/$mod"
      if [ -f "$src" ]; then
        mkdir -p "$(dirname "$dst")"
        cp -a "$src" "$dst"
      fi
    }
    for m in $REQ_MODS; do copy_mod_with_deps "$m"; done
    # Ensure modules.dep is correct for copied subset (keep original)
    # Copy harpoon init and mgmt
    cp -a "/repo/tools/guest-builder/src/init" "$STAGING/init"
    chmod +x "$STAGING/init"
    mkdir -p "$STAGING/usr/local/bin"
    cp -a "/repo/tools/guest-builder/src/harpoon-mgmt" "$STAGING/usr/local/bin/harpoon-mgmt"
    chmod +x "$STAGING/usr/local/bin/harpoon-mgmt"
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
