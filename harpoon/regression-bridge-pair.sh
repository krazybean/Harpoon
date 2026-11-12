#!/bin/sh
# Bridge connection lifecycle (descriptor ownership, abandoned clients, half-close, stalled peers, shutdown, bounded probes) with
# socketpair test doubles. No VM, vsock, Docker or socket path is used. Usage: sh harpoon/regression-bridge-pair.sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/src"; cp "$ROOT/harpoon/regression-bridge-pair-test.swift" "$tmp/src/main.swift"
xcrun swiftc -module-cache-path "$tmp/mc" "$ROOT/harpoon/Sources/BridgePair.swift" "$tmp/src/main.swift" -o "$tmp/t" 2>&1 | grep -E "error" && { echo "bridge pair FAIL: did not compile" >&2; exit 1; } || true
"$tmp/t"
