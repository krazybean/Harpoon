#!/bin/bash
set -euo pipefail
# ponytail: build both guests on an Apple Silicon Mac, publish them as a GitHub pre-release
# asset, and point the Universal macOS DMG workflow at them so CI can build the .dmg.
# Prerequisites: Apple Silicon, Rosetta, Docker (x86_64 initramfs), gh logged in with repo access.
# Usage: tools/release/macos/publish-guest-assets.sh [--skip-build]
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
ASSET_NAME="harpoon-guest-assets.tar.gz"

for cmd in gh docker shasum; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "[publish-guest-assets] FAIL: $cmd not found" >&2; exit 1; }
done
[ "$(uname -m)" = "arm64" ] || { echo "[publish-guest-assets] FAIL: needs an Apple Silicon Mac (VZ builder + Rosetta)" >&2; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "[publish-guest-assets] FAIL: run 'gh auth login' first" >&2; exit 1; }

if [ "${1:-}" != "--skip-build" ]; then
  echo "[publish-guest-assets] building aarch64 guest..." >&2
  bash "$REPO_ROOT/tools/guest-builder/build.sh"
  echo "[publish-guest-assets] building x86_64 guest..." >&2
  HARPOON_GUEST_ARCH=x86_64 bash "$REPO_ROOT/tools/guest-builder/build.sh"
fi

OUT="$REPO_ROOT/dist/$ASSET_NAME"
bash "$REPO_ROOT/tools/guest-builder/pack-assets.sh" "$OUT" >/dev/null
SHA="$(shasum -a 256 "$OUT" | cut -d' ' -f1)"
TAG="guest-assets-${SHA:0:12}"

cd "$REPO_ROOT"
if ! gh release view "$TAG" >/dev/null 2>&1; then
  gh release create "$TAG" --prerelease --title "Harpoon guest assets ${SHA:0:12}" \
    --notes "Prebuilt aarch64 + x86_64 guest kernels, initramfs and root images for the Universal macOS DMG workflow. sha256 $SHA"
fi
gh release upload "$TAG" "$OUT" --clobber
gh variable set HARPOON_GUEST_ASSETS_TAG --body "$TAG"
gh variable set HARPOON_GUEST_ASSETS_SHA256 --body "$SHA"
echo "[publish-guest-assets] published $TAG ($SHA)" >&2

# Re-run the latest Universal macOS DMG run on this branch so it picks up the assets.
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
RUN_ID="$(gh run list --workflow universal-dmg.yml --branch "$BRANCH" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
if [ -n "$RUN_ID" ]; then
  gh run rerun "$RUN_ID"
  echo "[publish-guest-assets] re-running Universal macOS DMG run $RUN_ID on $BRANCH; the .dmg appears under its Artifacts" >&2
else
  echo "[publish-guest-assets] no Universal macOS DMG run on $BRANCH yet; push or run the workflow to build the .dmg" >&2
fi
