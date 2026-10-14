#!/bin/bash
set -euo pipefail
# ponytail: canonical root always comes from the disposable Alpine VZ builder.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="${HARPOON_ROOT_OUT:-$REPO_ROOT/assets/guest/harpoon-root.img}"

BUILDER=""
FORCE=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --builder) [ "${2:-}" = "vz" ] || { echo "[build-root] FAIL: only --builder vz is supported" >&2; exit 2; }; BUILDER=vz; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) echo "usage: $0 --builder vz [--force]" >&2; exit 2 ;;
  esac
done
if [ "$BUILDER" != "vz" ]; then
  echo "[build-root] FAIL: explicit --builder vz is required" >&2
  echo "[build-root] This path deliberately does not reuse an existing image or use Docker/Podman." >&2
  exit 1
fi

if [ "$FORCE" -ne 1 ] && [ -f "$OUT" ]; then
  echo "[build-root] FAIL: refusing historical/cached root image; use --force for a clean rebuild" >&2
  exit 1
fi

bash "$SCRIPT_DIR/build-root-vz.sh"
bash "$SCRIPT_DIR/verify-root.sh" "$OUT"
