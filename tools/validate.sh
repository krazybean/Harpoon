#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

say() {
  printf '\n==> %s\n' "$*"
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "ERROR: required tool not found: $1" >&2
    exit 127
  }
}

need xcrun
need node
need npm
need cargo

case "$(uname -s)" in
  Darwin) ;;
  *)
    echo "ERROR: Harpoon validation requires macOS because the runtime links Virtualization.framework." >&2
    exit 2
    ;;
esac

NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]')"
if [ "$NODE_MAJOR" != "20" ]; then
  echo "ERROR: Harpoon desktop validation requires Node 20; found $(node --version)." >&2
  echo "See docs/building.md for the supported Node/NVM setup." >&2
  exit 2
fi

say "Build Harpoon runtime"
bash harpoon/build.sh

say "Run CLI parity regression suite"
HARPOON_BIN="$ROOT/harpoon/build/harpoon" sh harpoon/cli-parity-test.sh

say "Run host path translation regression suite"
sh harpoon/regression-host-path.sh

say "Run deterministic host path fuzz suite"
sh harpoon/fuzz-host-path.sh

UI_DIR="$ROOT/ui/harpoon-desktop"

say "Run release tooling preflight"
sh tools/release-preflight.sh

say "Install reproducible frontend dependencies when needed"
if [ ! -d "$UI_DIR/node_modules" ]; then
  (
    cd "$UI_DIR"
    npm ci
  )
fi

say "Run frontend unit tests"
(
  cd "$UI_DIR"
  npm test
)

say "Build frontend"
(
  cd "$UI_DIR"
  npm run build
)

say "Check Tauri Rust backend"
cargo check --locked --manifest-path "$UI_DIR/src-tauri/Cargo.toml"

printf '\nHARPOON_VALIDATION_PASS\n'
