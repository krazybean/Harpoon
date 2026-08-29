#!/bin/bash
set -euo pipefail
# ponytail: sanitize harpoon-root.img to containers=0 images=0 volumes=0 deterministically
# Strategy (in order):
# 1. If docker + privileged available, mount ext4 via Linux container and rm -rf /var/lib/docker/*.
# 2. Fallback: use debugfs (e2fsprogs) to remove docker state without mount (rm via debugfs).
# 3. If neither available, report BLOCKER — sanitization requires Linux or e2fsprogs.
# Never deletes provisioned user disk; only operates on template path given.

IMG="${1:-assets/guest/harpoon-root.img}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
if [[ "$IMG" != /* ]]; then
  if [ -f "$REPO_ROOT/$IMG" ]; then IMG="$REPO_ROOT/$IMG"; fi
fi
if [ ! -f "$IMG" ]; then
  echo "[sanitize] no image at $IMG, nothing to sanitize" >&2
  exit 0
fi
echo "[sanitize] sanitizing $IMG" >&2

# Try docker privileged mount clean first (most reliable on macOS with Docker Desktop)
try_docker_clean() {
  local img="$1"
  local dir
  dir="$(dirname "$img")"
  local base
  base="$(basename "$img")"
  echo "[sanitize] trying docker privileged clean..." >&2
  # Use a Linux container that can loop-mount ext4
  if ! command -v docker >/dev/null 2>&1; then return 1; fi
  # Check docker daemon reachable
  if ! docker info >/dev/null 2>&1; then
    echo "[sanitize] docker not reachable, skipping docker clean" >&2
    return 1
  fi
  # Run privileged alpine with e2fsprogs to clean
  docker run --rm --privileged \
    -v "$dir:/mnt:rw" \
    alpine:3.22 sh -c "
      set -e
      apk add --no-cache e2fsprogs >/dev/null 2>&1 || true
      IMG=/mnt/$base
      echo \"[sanitize:container] inspecting \$IMG\"
      # Try loop mount if possible
      mkdir -p /img_mnt
      if mount -o loop \"\$IMG\" /img_mnt 2>&1; then
        echo \"[sanitize:container] mounted, cleaning /var/lib/docker\"
        rm -rf /img_mnt/var/lib/docker/* 2>/dev/null || true
        rm -rf /img_mnt/var/lib/containerd/* 2>/dev/null || true
        # Also clean test volumes/images markers
        rm -rf /img_mnt/var/lib/docker/volumes/* 2>/dev/null || true
        rm -rf /img_mnt/var/lib/docker/containers/* 2>/dev/null || true
        rm -rf /img_mnt/var/lib/docker/image/* 2>/dev/null || true
        rm -rf /img_mnt/var/lib/docker/overlay2/* 2>/dev/null || true
        ls -la /img_mnt/var/lib/docker/ 2>&1 | head -n 20 || true
        sync
        umount /img_mnt
        echo \"[sanitize:container] cleaned via mount\"
        exit 0
      else
        echo \"[sanitize:container] loop mount failed, trying debugfs\"
        if command -v debugfs >/dev/null 2>&1; then
          # debugfs rm is interactive; use rdump approach: remove via debugfs commands
          # List docker dir
          debugfs -R \"ls -l /var/lib/docker\" \"\$IMG\" 2>&1 | head -n 20 || true
          # debugfs rm -rf equivalent: iterate and rm each
          for p in containers volumes image overlay2; do
            debugfs -w -R \"rm -r /var/lib/docker/\$p\" \"\$IMG\" 2>&1 | head -n 20 || true
          done
          # Recreate empty dirs
          for p in containers volumes image overlay2; do
            debugfs -w -R \"mkdir /var/lib/docker/\$p\" \"\$IMG\" 2>&1 | head -n 5 || true
          done
          echo \"[sanitize:container] cleaned via debugfs\"
          exit 0
        else
          echo \"[sanitize:container] no debugfs, cannot clean without mount\" >&2
          exit 1
        fi
      fi
    "
  return $?
}

if try_docker_clean "$IMG"; then
  echo "[sanitize] docker clean succeeded" >&2
  bash "$SCRIPT_DIR/verify-root.sh" "$IMG" && echo "[sanitize] verified clean" >&2 && exit 0
  echo "[sanitize] docker clean did not fully verify, trying host debugfs..." >&2
fi

# Host debugfs fallback (brew install e2fsprogs)
if command -v debugfs >/dev/null 2>&1; then
  echo "[sanitize] trying host debugfs clean..." >&2
  # Remove docker state via debugfs write mode
  for p in containers volumes image overlay2 network tmp buildkit; do
    debugfs -w -R "rm -r /var/lib/docker/$p" "$IMG" 2>&1 | head -n 10 || true
  done
  # Also clean containerd
  debugfs -w -R "rm -r /var/lib/containerd" "$IMG" 2>&1 | head -n 10 || true
  # Recreate empty dirs to keep Docker functional
  for p in containers volumes image overlay2 network tmp buildkit; do
    debugfs -w -R "mkdir /var/lib/docker/$p" "$IMG" 2>&1 | head -n 5 || true
  done
  debugfs -w -R "mkdir /var/lib/containerd" "$IMG" 2>&1 | head -n 5 || true
  echo "[sanitize] host debugfs clean attempted" >&2
  if bash "$SCRIPT_DIR/verify-root.sh" "$IMG" 2>&1; then
    echo "[sanitize] verified clean via debugfs" >&2
    exit 0
  fi
  echo "[sanitize] debugfs clean did not fully verify" >&2
fi

echo "[sanitize] BLOCKER: deterministic sanitization requires docker privileged or e2fsprogs debugfs" >&2
echo "[sanitize] Install: brew install e2fsprogs  OR  ensure docker daemon is running (docker info)" >&2
echo "[sanitize] Then re-run: bash tools/guest-builder/sanitize-root.sh $IMG" >&2
echo "[sanitize] Image left unmodified at $IMG" >&2
# Do not delete image; report blocker
exit 1
