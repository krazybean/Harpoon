#!/bin/bash
set -euo pipefail
# ponytail: create/sanitize canonical harpoon-root.img — deterministic, no bootstrap cycle
# Precedence:
#   1. explicit FETCH env (HARPOON_ROOT_URL/SHA256) — versioned artifact
#   2. local canonical if already built (assets/guest/harpoon-root.img 2147483648, 0/0/0)
#   3. deterministic REBUILD via Docker Linux (qemu-img + mkfs.ext4)
#   4. BOOTSTRAP only if HARPOON_ALLOW_BOOTSTRAP=1 (development-only, not release)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="$REPO_ROOT/assets/guest/harpoon-root.img"
BOOTSTRAP="$REPO_ROOT/assets/guest/.bootstrap/harpoon-root.img"
mkdir -p "$REPO_ROOT/assets/guest"

is_canonical_size() {
  local f="$1"
  [ -f "$f" ] || return 1
  local sz
  sz=$(stat -f%z "$f" 2>/dev/null || stat -c%s "$f")
  [ "$sz" = "2147483648" ]
}

if is_canonical_size "$OUT"; then
  echo "[build-root] canonical root already exists at $OUT ($(du -h "$OUT" | awk '{print $1}') physical)" >&2
  if [ "${1:-}" != "--force" ]; then
    if bash "$SCRIPT_DIR/verify-root.sh" "$OUT" 2>&1; then
      echo "[build-root] verification passed, nothing to do" >&2
      exit 0
    else
      echo "[build-root] verification failed, attempting sanitize..." >&2
      bash "$SCRIPT_DIR/sanitize-root.sh" "$OUT" || true
      if bash "$SCRIPT_DIR/verify-root.sh" "$OUT" 2>&1; then
        echo "[build-root] sanitize succeeded" >&2
        exit 0
      else
        echo "[build-root] sanitize did not fully clean; manual intervention required" >&2
        exit 1
      fi
    fi
  fi
fi

# --- FETCH MODE ---
RELEASE_URL="${HARPOON_ROOT_URL:-}"
RELEASE_SHA="${HARPOON_ROOT_SHA256:-}"
DEFAULT_RELEASE_URL="https://github.com/Harpoon/releases/download/v0.1.1/harpoon-root.img"
DEFAULT_SHA="" # TODO: populate after v0.1.1 publish
CACHE_DIR="$REPO_ROOT/assets/guest/.cache"
mkdir -p "$CACHE_DIR"

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
  echo "[build-root] FETCH MODE: fetching $FETCH_URL" >&2
  TMP_FETCH="$CACHE_DIR/harpoon-root.img.tmp"
  curl -L --fail -o "$TMP_FETCH" "$FETCH_URL"
  ACTUAL_SHA=$(shasum -a 256 "$TMP_FETCH" | cut -d' ' -f1)
  if [ "$ACTUAL_SHA" != "$FETCH_SHA" ]; then
    echo "[build-root] FAIL: SHA mismatch expected $FETCH_SHA got $ACTUAL_SHA" >&2
    rm -f "$TMP_FETCH"
    exit 1
  fi
  SZ=$(stat -f%z "$TMP_FETCH" 2>/dev/null || stat -c%s "$TMP_FETCH")
  if [ "$SZ" != "2147483648" ]; then
    echo "[build-root] FAIL: fetched size $SZ != 2147483648" >&2
    rm -f "$TMP_FETCH"
    exit 1
  fi
  mv "$TMP_FETCH" "$OUT"
  echo "[build-root] fetched and verified $OUT (sha256 $ACTUAL_SHA)" >&2
  ls -lh "$OUT" >&2; du -h "$OUT" >&2
  bash "$SCRIPT_DIR/verify-root.sh" "$OUT" || {
    echo "[build-root] FAIL: fetched image failed sanitation check" >&2
    exit 1
  }
  exit 0
fi

# --- REBUILD MODE (deterministic, Docker Linux) ---
echo "[build-root] REBUILD MODE: deterministic 2G sparse ext4..." >&2
# Use Docker to ensure deterministic mkfs.ext4 with fixed UUID and no random
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  echo "[build-root] Docker available, creating via Alpine..." >&2
  if docker run --rm --privileged -v "$REPO_ROOT/assets/guest:/out" alpine:3.22 sh -c '
    set -e
    apk add --no-cache e2fsprogs qemu-img > /dev/null 2>&1
    # Create sparse 2G raw with fixed UUID, no discard, deterministic
    # Use truncate for sparse, then mkfs.ext4 with fixed UUID and label, no lazy init randomness
    # qemu-img create is sparse on Linux; use truncate + mkfs for determinism
    rm -f /out/harpoon-root.img
    truncate -s 2147483648 /out/harpoon-root.img
    mkfs.ext4 -L harpoon-root -U 00000000-0000-0000-0000-000000000001 -E lazy_itable_init=0,lazy_journal_init=0 -q /out/harpoon-root.img
    # Verify
    dumpe2fs -h /out/harpoon-root.img 2>&1 | grep -E "Block count|UUID|Filesystem volume" | head -n 10
    echo "created $(ls -lh /out/harpoon-root.img | awk '\''{print $9, $5}'\'')"
    du -h /out/harpoon-root.img
  '; then
    echo "[build-root] created via Docker" >&2
    ls -lh "$OUT" >&2; du -h "$OUT" >&2
    bash "$SCRIPT_DIR/verify-root.sh" "$OUT" || {
      echo "[build-root] FAIL: rebuilt image failed sanitation (should be 0/0/0 empty)" >&2
      exit 1
    }
    if command -v shasum >/dev/null; then echo "[build-root] sha256 $(shasum -a 256 "$OUT" | cut -d' ' -f1)" >&2; fi
    exit 0
  else
    echo "[build-root] Docker rebuild failed" >&2
  fi
fi

# Native qemu-img/mkfs if available (Linux)
if command -v qemu-img >/dev/null 2>&1 && command -v mkfs.ext4 >/dev/null 2>&1; then
  echo "[build-root] creating 2G raw via qemu-img (native)..." >&2
  rm -f "$OUT"
  truncate -s 2147483648 "$OUT"
  mkfs.ext4 -L harpoon-root -U 00000000-0000-0000-0000-000000000001 -E lazy_itable_init=0,lazy_journal_init=0 -q "$OUT"
  echo "[build-root] created $OUT" >&2
  ls -lh "$OUT" >&2; du -h "$OUT" >&2
  bash "$SCRIPT_DIR/verify-root.sh" "$OUT" || exit 1
  exit 0
fi

# --- BOOTSTRAP only if explicitly allowed (development-only) ---
if [ "${HARPOON_ALLOW_BOOTSTRAP:-}" = "1" ] && [ -f "$BOOTSTRAP" ]; then
  echo "[build-root] BOOTSTRAP MODE (development-only, HARPOON_ALLOW_BOOTSTRAP=1): using $BOOTSTRAP" >&2
  if cp -c "$BOOTSTRAP" "$OUT" 2>/dev/null; then :;
  elif ditto "$BOOTSTRAP" "$OUT" 2>/dev/null; then :;
  else cp -p "$BOOTSTRAP" "$OUT"; fi
  ls -lh "$OUT" >&2; du -h "$OUT" >&2
  echo "[build-root] seeded via bootstrap; now sanitizing..." >&2
  bash "$SCRIPT_DIR/sanitize-root.sh" "$OUT" || true
  bash "$SCRIPT_DIR/verify-root.sh" "$OUT" || {
    echo "[build-root] WARNING: bootstrap image failed sanitation check" >&2
    exit 1
  }
  echo "[build-root] done (bootstrap, not for release)" >&2
  exit 0
fi

if [ -f "$OUT" ] && ! is_canonical_size "$OUT"; then
  echo "[build-root] existing $OUT has wrong size (expected 2147483648)" >&2
  exit 1
fi

echo "[build-root] FAIL: cannot obtain $OUT on fresh clone" >&2
echo "[build-root] No FETCH artifact (set HARPOON_ROOT_URL/SHA256) and REBUILD requires Docker (docker info must succeed)" >&2
echo "[build-root] No BOOTSTRAP at $BOOTSTRAP (requires HARPOON_ALLOW_BOOTSTRAP=1)" >&2
echo "[build-root] BLOCKER: fresh clone requires Docker to rebuild harpoon-root.img" >&2
exit 1
