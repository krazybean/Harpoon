#!/bin/sh
# Which guest address the engine may publish as the forwarder's target, and the shape it is published in.
# Pure unit test: compiles ForwardTarget.swift with a test main; no VM, no Docker, no engine.
set -eu
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp harpoon/regression-forward-target.swift "$tmp/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" harpoon/Sources/ForwardTarget.swift "$tmp/main.swift" -o "$tmp/test"
"$tmp/test"
