#!/bin/sh
set -eu
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp harpoon/regression-host-path.swift "$tmp/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" harpoon/Sources/HostPathTranslator.swift "$tmp/main.swift" -o "$tmp/test"
"$tmp/test"
