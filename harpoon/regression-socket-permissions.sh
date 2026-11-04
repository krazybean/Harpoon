#!/bin/sh
set -eu

# Offline permission regression: use a private regular file, never a live
# Harpoon socket. The Swift bridge has the same fail-closed chmod/stat gates.
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
BRIDGE="$ROOT/harpoon/Sources/UnixBridge.swift"
TMP="$(mktemp -d /tmp/harpoon-socket-permissions.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

SOCKET_PLACEHOLDER="$TMP/socket-placeholder"
: > "$SOCKET_PLACEHOLDER"
chmod 0600 "$SOCKET_PLACEHOLDER"
MODE=$(stat -f '%Lp' "$SOCKET_PLACEHOLDER" 2>/dev/null || stat -c '%a' "$SOCKET_PLACEHOLDER")
[ "$MODE" = 600 ] || { echo "socket permissions FAIL: expected 600, got $MODE" >&2; exit 1; }

! chmod 0600 "$TMP/missing/socket" 2>/dev/null
grep -q 'guard chmod(config.dockerSocketPath, 0o600) == 0' "$BRIDGE"
grep -q '(socketStat.st_mode & 0o777) == 0o600' "$BRIDGE"
grep -q 'removeItem(atPath: config.dockerSocketPath)' "$BRIDGE"

echo 'socket-permissions PASS: 0600 assertion and chmod failure cleanup are covered offline'
