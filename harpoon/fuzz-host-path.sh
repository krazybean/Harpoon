#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cp harpoon/fuzz-host-path.swift "$tmp/main.swift"
xcrun swiftc \
  -module-cache-path "$tmp/module-cache" \
  harpoon/Sources/HostPathTranslator.swift \
  "$tmp/main.swift" \
  -o "$tmp/host-path-fuzz"

"$tmp/host-path-fuzz"
