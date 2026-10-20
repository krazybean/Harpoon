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
. "$SCRIPT_DIR/guest-arch.sh"
ASSET_DIR="$(guest_asset_dir "$REPO_ROOT/assets/guest")"
OUT="${HARPOON_INITRAMFS_OUT:-$ASSET_DIR/harpoon-initramfs.cpio.gz}"
FINGERPRINT="$OUT.inputs"
BOOTSTRAP="$REPO_ROOT/assets/guest/.bootstrap/harpoon-initramfs.cpio.gz"
CACHE_DIR="$ASSET_DIR/.cache"
SRC_INIT="$REPO_ROOT/tools/guest-builder/src/init"
SRC_MGMT="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt"
SRC_MGMT_WRAPPER="$REPO_ROOT/tools/guest-builder/src/harpoon-mgmt-wrapper"
ROOT_IMG="$ASSET_DIR/harpoon-root.img"
source "$SCRIPT_DIR/build-engine.sh"
mkdir -p "$ASSET_DIR" "$CACHE_DIR"

# Pinned production inputs (per guest arch in guest-arch.sh) — mismatch FAILs
EXPECTED_KERNEL="$GUEST_KERNEL_VERSION"

input_manifest() {
  local file rel
  printf 'harpoon-initramfs-inputs-v1\n'
  [ "$HARPOON_GUEST_ARCH" = "aarch64" ] || printf 'pin %s %s\n' arch "$HARPOON_GUEST_ARCH"
  printf 'pin %s %s\n' kernel "$EXPECTED_KERNEL"
  printf 'pin %s %s\n' vmlinuz "$EXPECTED_VMLINUZ_SHA"
  printf 'pin %s %s\n' initramfs-virt "$EXPECTED_INITRAMFS_VIRT_SHA"
  printf 'pin %s %s\n' modloop "$EXPECTED_MODLOOP_SHA"
  printf 'pin %s %s\n' minirootfs "$EXPECTED_MINIROOTFS_SHA"
  for file in "$SCRIPT_DIR/build-initramfs.sh" "$SCRIPT_DIR/rebuild-initramfs-container.sh" "$SRC_INIT" "$SRC_MGMT" "$SRC_MGMT_WRAPPER" "$REPO_ROOT/tools/guest-builder/required-modules.txt" "$REPO_ROOT/tools/guest-builder/required-kernel-features.txt" "$REPO_ROOT/tools/guest-builder/kernel-feature-modules.txt" "$ROOT_IMG"; do
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
for _f in "$CACHE_DIR/vmlinuz-virt" "$CACHE_DIR/initramfs-virt" "$CACHE_DIR/modloop-virt" "$CACHE_DIR/$GUEST_MINIROOTFS"; do
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
# Pinned inputs (Alpine 3.22, kernel 6.12.110-0-virt; aarch64 shown, every arch pinned in guest-arch.sh):
#   Kernel: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/vmlinuz-virt (6.12.110-0-virt)
#   Initramfs-virt: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/initramfs-virt
#   Modloop: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/modloop-virt
#   Minirootfs: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/alpine-minirootfs-3.22.1-aarch64.tar.gz
# Packages pinned at build time via apk add with explicit Alpine 3.22 repos (no latest).
# Required modules injected from modloop via modules.dep dependency walk (virtio, vsock, etc).
# SHA-256 for upstream artifacts where practical (recorded after fetch, verified on rebuild):
#   vmlinuz-virt: dd37c4460f933ec909ff022c34944ff7e1a5300aff99750e3bdc02cd4074c332 (cached .cache/vmlinuz-virt)
#   initramfs-virt: 384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c (cached)
#   modloop-virt: (fetched, sha printed at build)
#   alpine-minirootfs-3.22.1: (fetched, sha printed)

if harpoon_detect_build_engine; then
  echo "[build-initramfs] build engine: $HARPOON_BUILD_ENGINE_DESCRIPTION" >&2
  echo "[build-initramfs] REBUILD MODE: deterministic rebuild via Docker (Alpine 3.22)..." >&2
  # Ensure src init/mgmt exist (committed)
  if [ ! -f "$SRC_INIT" ]; then echo "[build-initramfs] FAIL: missing $SRC_INIT (committed init source)" >&2; exit 1; fi
  if [ ! -f "$SRC_MGMT" ]; then echo "[build-initramfs] FAIL: missing $SRC_MGMT" >&2; exit 1; fi
  # Run rebuild inside Alpine container
  harpoon_build_engine run --rm --platform "$GUEST_CONTAINER_PLATFORM" -e HARPOON_GUEST_ARCH="$HARPOON_GUEST_ARCH" \
    -v "$REPO_ROOT:/repo" -v "$CACHE_DIR:/cache" alpine:3.22 \
    sh /repo/tools/guest-builder/rebuild-initramfs-container.sh
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
echo "[build-initramfs] No FETCH artifact (set HARPOON_INITRAMFS_URL/SHA256) and REBUILD requires a reachable Docker-compatible engine" >&2
echo "[build-initramfs] No BOOTSTRAP at $BOOTSTRAP (requires HARPOON_ALLOW_BOOTSTRAP=1)" >&2
echo "[build-initramfs] BLOCKER: fresh clone requires an external Docker-compatible build engine to generate harpoon-initramfs.cpio.gz" >&2
exit 1
