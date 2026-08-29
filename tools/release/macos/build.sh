#!/bin/bash
set -euo pipefail
# ponytail: build release Harpoon.app from source (clean, deterministic, no stale bundle)
# Prerequisites: Xcode CLT (swiftc, codesign), Rust stable, Node 20, Docker (for guest REBUILD if needed)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
VERSION="${HARPOON_VERSION:-$(node -p "require('$REPO_ROOT/ui/harpoon-desktop/package.json').version" 2>/dev/null || echo "0.1.1")}"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then echo "[build] FAIL: invalid version $VERSION" >&2; exit 1; fi
echo "[build] Harpoon v$VERSION clean build" >&2
# 1. verify prerequisites
for cmd in swiftc codesign cargo node npm docker; do
  if ! command -v "$cmd" >/dev/null 2>&1; then echo "[build] FAIL: $cmd not found" >&2; exit 1; fi
done
# Docker check: need at least one engine (Harpoon or Desktop) for guest REBUILD if assets missing
if ! docker info >/dev/null 2>&1 && ! docker --context harpoon info >/dev/null 2>&1; then
  echo "[build] WARN: docker engine not running — guest REBUILD will require external Docker (first build needs external engine)" >&2
fi
# 2. obtain/build canonical guest assets
echo "[build] guest assets..." >&2
bash "$REPO_ROOT/tools/guest-builder/build.sh" 2>&1 | tail -n 20
# 3. verify pristine root
bash "$REPO_ROOT/tools/guest-builder/verify-root.sh" 2>&1 | tail -n 10
# 4. build Swift runtime
echo "[build] Swift runtime..." >&2
bash "$REPO_ROOT/harpoon/build.sh" 2>&1 | tail -n 20
# 5. prepare Tauri resources (bundle-resources)
echo "[build] prepare Tauri resources..." >&2
bash "$REPO_ROOT/ui/harpoon-desktop/src-tauri/prepare-bundle.sh" 2>&1 | tail -n 10
# 6. build frontend
echo "[build] frontend..." >&2
npm --prefix "$REPO_ROOT/ui/harpoon-desktop" run build 2>&1 | tail -n 20
# 7. build release Harpoon.app (Tauri)
echo "[build] Tauri bundle..." >&2
# Use tauri build with explicit version (sets tauri.conf.json via version.mjs if needed)
# Ensure version consistency before build
node "$REPO_ROOT/ui/harpoon-desktop/scripts/version.mjs" --check 2>&1 | tail -n 10 || true
# Build release bundle
npm --prefix "$REPO_ROOT/ui/harpoon-desktop" run tauri -- build --bundles app 2>&1 | tail -n 30
APP="$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app"
if [ ! -d "$APP" ]; then echo "[build] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
# 8. structural verify (pre-signing, ad-hoc is ok)
echo "[build] structural verify..." >&2
bash "$REPO_ROOT/tools/verify-bundle.sh" "$APP" 2>&1 | tail -n 30
echo "[build] DONE Harpoon.app at $APP (v$VERSION, ad-hoc, not yet Developer ID)" >&2
ls -lh "$APP" >&2
