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
bash -n "$ROOT/tools/check-journal-readers.sh"
bash -n "$ROOT/tools/check-release-bundle-capabilities.sh"
bash -n "$ROOT/tools/release/macos/verify-release-assets.sh"
bash -n "$ROOT/tools/release/macos/sign-existing-dmg.sh"
bash -n "$ROOT/tools/release/macos/regression-sign-existing-dmg.sh"

say "Check the release verification tooling on synthetic bundles and a mocked GitHub (nothing is built or published)"
sh "$ROOT/tools/regression-release-capabilities.sh"
bash "$ROOT/tools/release/macos/regression-release-assets.sh"

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

say "Verify the packaged resources are still declared, and the debug-only copy skip stays gated"
# build.rs asks tauri-build to copy no resources, but only in a build with debug assertions and only through
# TAURI_CONFIG. The packaged app must keep declaring the whole resource tree in tauri.conf.json.
node -e '
  const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const r = c.bundle && c.bundle.resources;
  if (!r || Array.isArray(r) || r["bundle-resources/harpoon"] !== "harpoon") {
    console.error("ERROR: tauri.conf.json no longer maps bundle-resources/harpoon to harpoon");
    process.exit(1);
  }
' "$UI_DIR/src-tauri/tauri.conf.json"
grep -q 'CARGO_CFG_DEBUG_ASSERTIONS' "$UI_DIR/src-tauri/build.rs" || {
  echo "ERROR: build.rs no longer gates its resource-copy skip on debug assertions" >&2
  exit 1
}
# the one place build.rs sets TAURI_CONFIG; a second would be a skip nobody reviewed
[ "$(grep -c 'set_var("TAURI_CONFIG"' "$UI_DIR/src-tauri/build.rs")" = 1 ] || {
  echo "ERROR: build.rs must set TAURI_CONFIG exactly once, inside the debug-assertions guard" >&2
  exit 1
}

printf '\nHARPOON_RELEASE_PREFLIGHT_PASS\n'
