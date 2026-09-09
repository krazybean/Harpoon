#!/bin/bash
set -euo pipefail

BIN=${1:-harpoon/build/harpoon}
ROOT=assets/guest/harpoon-root.img
TMP=$(mktemp -d /tmp/harpoon-foreground-status.XXXXXX)
RUNTIME="$TMP/runtime"
DISK="$TMP/root.img"
SOCK=/tmp/harpoon-docker.sock
MGMT=/tmp/harpoon-mgmt.sock
CONTROL=/tmp/harpoon-control
LOCK=/tmp/harpoon.lock
RUN_PID=
SOCKET_OWNED=0

fail() { echo "foreground-status FAIL: $*" >&2; exit 1; }
state_is() {
  "$BIN" status --json | python3 -c '
import json, sys
state, ready = sys.argv[1:]
d = json.load(sys.stdin)
assert d["state"] == state, d
assert str(d["dockerReady"]).lower() == ready, d
' "$1" "$2"
}
stop_run() {
  [ -n "$RUN_PID" ] || return 0
  kill -TERM "$RUN_PID" 2>/dev/null || true
  wait "$RUN_PID" 2>/dev/null || true
  RUN_PID=
}
cleanup() {
  local rc=$?
  stop_run
  [ -f "$RUNTIME/runtime.pid" ] && "$BIN" stop >/dev/null 2>&1 || true
  [ "$SOCKET_OWNED" -eq 1 ] && rm -f "$SOCK"
  rm -rf "$TMP"
  exit "$rc"
}
trap cleanup EXIT

test -x "$BIN" || fail "missing executable: $BIN"
test -f "$ROOT" || fail "missing root template: $ROOT"
for path in "$SOCK" "$MGMT" "$CONTROL"; do [ ! -e "$path" ] || fail "existing Harpoon socket: $path"; done
if [ -e "$LOCK" ] && ! perl -e 'open my $f, "+<", $ARGV[0] or exit 0; flock($f, 2) or exit 1' "$LOCK"; then
  fail "Harpoon lock is held"
fi

export HARPOON_TEST_TMPDIR="$RUNTIME"
export HARPOON_ALLOW_TMP_FALLBACK=1
export HARPOON_DISK="$DISK"
(cp -c "$ROOT" "$DISK" 2>/dev/null || cp "$ROOT" "$DISK") || fail "cannot clone root"

echo "foreground-status: direct run"
"$BIN" run >"$TMP/run.log" 2>&1 &
RUN_PID=$!
for _ in $(seq 1 120); do
  [ ! -e "$RUNTIME/runtime.pid" ] && [ ! -e "$RUNTIME/runtime.json" ] || fail "direct run wrote launcher metadata"
  if state_is running true 2>/dev/null; then break; fi
  kill -0 "$RUN_PID" 2>/dev/null || { cat "$TMP/run.log" >&2; fail "direct run exited"; }
  sleep 1
done
state_is running true || fail "direct run classified running"
docker --context harpoon version >/dev/null 2>&1 || fail "direct run Docker readiness"
stop_run
state_is stopped false || fail "direct run stopped state"

mkdir -p "$RUNTIME"
printf '99999999\n' > "$RUNTIME/runtime.pid"
state_is stale false || fail "stale PID state"
rm -f "$RUNTIME/runtime.pid"

python3 -c 'import os, socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); os.chmod(sys.argv[1], 0o600)' "$SOCK"
SOCKET_OWNED=1
[ -S "$SOCK" ] || fail "orphan socket did not start"
state_is degraded false || fail "orphan socket degraded state"
rm -f "$SOCK"
SOCKET_OWNED=0

echo "foreground-status: normal start"
"$BIN" start >/dev/null || fail "normal start"
SOCKET_OWNED=1
[ -s "$RUNTIME/runtime.pid" ] && [ -s "$RUNTIME/runtime.json" ] || fail "normal start metadata"
state_is running true || fail "normal start running state"
"$BIN" stop >/dev/null || fail "normal stop"
SOCKET_OWNED=0
state_is stopped false || fail "normal stopped state"

echo "foreground-status PASS: foreground lock fallback and normal lifecycle states"
