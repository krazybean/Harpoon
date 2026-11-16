#!/bin/sh
# Engine ownership regression. Only ever runs the built binary inside throwaway namespaces (HARPOON_TEST_TMPDIR) and never
# boots a VM: `run` is used only against an occupied lock, where it must exit before any VM work. Proves that
#   - a second engine on an occupied namespace is rejected (exit 10) and leaves the holder's lock file alone,
#   - an independent namespace is unaffected by it,
#   - a lock file nobody holds, or a stale pid, is not proof of a live engine, and no unrelated process is signalled,
#   - none of it touches the real engine's lock.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="${HARPOON_BIN:-$SCRIPT_DIR/build/harpoon}"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }
[ -x "$BIN" ] || fail "Harpoon binary not found/executable: $BIN"

PROD_LOCK=/tmp/harpoon.lock
prod_fp() { if [ -e "$PROD_LOCK" ]; then stat -f '%i %m %z' "$PROD_LOCK"; else echo absent; fi; }
BEFORE="$(prod_fp)"

TMP="$(mktemp -d /tmp/harpoon-ownership.XXXXXX)"
HOLD_PID=""; BYSTANDER=""
cleanup() {
  for p in "$HOLD_PID" "$BYSTANDER"; do
    [ -n "$p" ] || continue
    kill "$p" 2>/dev/null || true
    wait "$p" 2>/dev/null || true
  done
  rm -rf "$TMP"
}
trap cleanup EXIT
mkdir -p "$TMP/a" "$TMP/b" "$TMP/c"
unset HARPOON_DOCKER_SOCK HARPOON_DISK HARPOON_TEST_MODE HARPOON_ALLOW_TMP_FALLBACK || true

status_field() { # namespace, field
  env HARPOON_TEST_TMPDIR="$TMP/$1" "$BIN" status --json 2>/dev/null |
    python3 -I -c "import sys,json; print(json.load(sys.stdin).get(sys.argv[1]))" "$2"
}

# --- an engine holds namespace a ------------------------------------------------------------------------------------
python3 -I -c "
import fcntl, os, time
fd = os.open('$TMP/a/harpoon.lock', os.O_CREAT | os.O_RDWR, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
open('$TMP/a/held', 'w').close()
time.sleep(120)" &
HOLD_PID=$!
i=0; while [ ! -e "$TMP/a/held" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
[ -e "$TMP/a/held" ] || fail "the stand-in engine never took its lock"
INODE="$(stat -f '%i' "$TMP/a/harpoon.lock")"

# 1. A second engine on the occupied namespace is rejected, before any VM work, and the holder's lock is untouched.
set +e
OUT="$(env HARPOON_TEST_TMPDIR="$TMP/a" "$BIN" run --disk "$TMP/never-used.img" 2>&1)"; CODE=$?
set -e
[ "$CODE" -eq 10 ] || fail "second engine: expected exit 10, got $CODE: $OUT"
printf '%s' "$OUT" | grep -q "HARPOON_ALREADY_RUNNING" || fail "second engine: no HARPOON_ALREADY_RUNNING: $OUT"
printf '%s' "$OUT" | grep -q "HARPOON_STATE" && fail "second engine got as far as a lifecycle state: $OUT"
[ "$(stat -f '%i' "$TMP/a/harpoon.lock")" = "$INODE" ] || fail "the rejected engine replaced the holder's lock file"
kill -0 "$HOLD_PID" 2>/dev/null || fail "the rejected engine ended the holder"
[ "$(status_field a lockHeld)" = "True" ] || fail "the holder's namespace no longer reads as held"
pass "a second engine on an occupied namespace is rejected and leaves the holder's lock alone"

# 2. An independent namespace is not affected by it.
[ "$(status_field b lockHeld)" = "False" ] || fail "namespace b reads as held because a is"
[ "$(status_field b state)" = "stopped" ] || fail "namespace b is not stopped"
case "$(status_field b lockPath)" in "$TMP/b/harpoon.lock") ;; *) fail "namespace b has a lock outside its root" ;; esac
pass "an independent namespace keeps its own lock and state"

# 3. A lock file nobody holds, with a pid file naming a live unrelated process, is stale, not a running engine; nothing is signalled.
sleep 300 &
BYSTANDER=$!
echo "$BYSTANDER" > "$TMP/c/runtime.pid"
: > "$TMP/c/harpoon.lock"
[ "$(status_field c lockHeld)" = "False" ] || fail "an unheld lock file reads as held"
[ "$(status_field c state)" = "stale" ] || fail "a stale pid reads as $(status_field c state), not stale"
kill -0 "$BYSTANDER" 2>/dev/null || fail "reading a stale pid signalled the unrelated process it names"
pass "an unheld lock file and a stale pid are not a live engine, and the process they name is left alone"

[ "$(prod_fp)" = "$BEFORE" ] || fail "the real engine's lock changed: $BEFORE -> $(prod_fp)"
pass "the real engine's lock is identical before and after"
