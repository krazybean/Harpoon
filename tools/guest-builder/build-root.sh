#!/bin/bash
set -euo pipefail
# ponytail: canonical root always comes from the disposable Alpine VZ builder.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="${HARPOON_ROOT_OUT:-$REPO_ROOT/assets/guest/harpoon-root.img}"

if [ "${HARPOON_ROOT_BUILDER:-}" != "vz" ]; then
  echo "[build-root] FAIL: root population requires HARPOON_ROOT_BUILDER=vz" >&2
  echo "[build-root] This path deliberately does not reuse an existing image or use Docker/Podman." >&2
  exit 1
fi

if [ "${1:-}" != "--force" ] && [ -f "$OUT" ]; then
  echo "[build-root] FAIL: refusing historical/cached root image; use --force for a clean rebuild" >&2
  exit 1
fi

bash "$SCRIPT_DIR/build-root-vz.sh"
bash "$SCRIPT_DIR/verify-root.sh" "$OUT"
