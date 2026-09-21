#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
UI_DIR="$ROOT/ui/harpoon-desktop"

say() {
  printf '\n==> %s\n' "$*"
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "ERROR: required tool not found: $1" >&2
    exit 127
  }
}

need node
need npm
need bash

say "Check synchronized release versions"
(
  cd "$UI_DIR"
  npm run version:check
)

say "Syntax-check release JavaScript"
node --check "$UI_DIR/scripts/release.mjs"
node --check "$UI_DIR/scripts/version.mjs"
node --check "$UI_DIR/scripts/clean.mjs"

say "Syntax-check release shell tooling"
bash -n "$UI_DIR/src-tauri/prepare-bundle.sh"
bash -n "$UI_DIR/scripts/sign-app.sh"
bash -n "$ROOT/harpoon/build.sh"

say "Verify release entrypoints and tracked inputs"
for path in \
  "$UI_DIR/src-tauri/tauri.conf.json" \
  "$UI_DIR/src-tauri/Cargo.toml" \
  "$UI_DIR/src-tauri/prepare-bundle.sh" \
  "$UI_DIR/scripts/release.mjs" \
  "$UI_DIR/scripts/sign-app.sh" \
  "$ROOT/assets/guest/harpoon-initramfs.cpio.gz" \
  "$ROOT/assets/guest/harpoon-initramfs.cpio.gz.inputs"
do
  [ -f "$path" ] || {
    echo "ERROR: required tracked release input missing: ${path#$ROOT/}" >&2
    exit 1
  }
done

# The production kernel and sparse root image are intentionally not committed.
# A clean checkout must document how those external release inputs are supplied
# rather than pretending the final DMG can be assembled from tracked files alone.
grep -q 'Image-virt' "$UI_DIR/src-tauri/prepare-bundle.sh" || {
  echo "ERROR: prepare-bundle no longer references Image-virt" >&2
  exit 1
}
grep -q 'harpoon-root.img' "$UI_DIR/src-tauri/prepare-bundle.sh" || {
  echo "ERROR: prepare-bundle no longer references harpoon-root.img" >&2
  exit 1
}
grep -q 'harpoon-root.img' "$ROOT/assets/guest/.bootstrap/README.md" || {
  echo "ERROR: bootstrap documentation no longer covers harpoon-root.img" >&2
  exit 1
}

printf '\nHARPOON_RELEASE_PREFLIGHT_PASS\n'
