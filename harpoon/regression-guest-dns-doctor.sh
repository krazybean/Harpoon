#!/bin/sh
# Offline: compiles the doctor's guest-DNS logic with its test and runs it. No VM, no network.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$ROOT/harpoon/guest-dns-doctor-test.swift" "$tmp/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$ROOT/harpoon/Sources/GuestDNS.swift" "$tmp/main.swift" -o "$tmp/guest-dns-doctor"
"$tmp/guest-dns-doctor"
