#!/bin/sh
# Route selection for published ports (guest address vs guest loopback) and the guest relay commands.
# Pure unit test: compiles PortForwardManager.swift with a test main; no VM, no Docker, no engine.
set -eu
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp harpoon/regression-published-port-route.swift "$tmp/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" -framework Virtualization harpoon/Sources/PortForwardManager.swift "$tmp/main.swift" -o "$tmp/test"
"$tmp/test"
