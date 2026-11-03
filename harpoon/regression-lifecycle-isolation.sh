#!/bin/sh
# Lifecycle isolation regression. Runs the built harpoon binary ONLY inside a throwaway namespace
# (HARPOON_TEST_TMPDIR), against a fake engine it plants there. It never starts a VM, never touches
# Docker, and proves the real engine's files (/tmp/harpoon*, ~/Library/Application Support/Harpoon)
# are byte- and inode-identical before and after.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="${HARPOON_BIN:-$SCRIPT_DIR/build/harpoon}"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }
[ -x "$BIN" ] || fail "Harpoon binary not found/executable: $BIN"

PROD_SUPPORT="$HOME/Library/Application Support/Harpoon"
# A serving engine appends to its own logs, so those are compared by inode only (a start or a
# rotation replaces the file); everything else must be identical down to mtime and size.
fingerprint() {
  for f in /tmp/harpoon.lock /tmp/harpoon-lifecycle.lock /tmp/harpoon-docker.sock /tmp/harpoon-control \
           /tmp/harpoon-mgmt.sock /tmp/harpoon-stop \
           "$PROD_SUPPORT/runtime.pid" "$PROD_SUPPORT/runtime.json" "$PROD_SUPPORT/config.json"; do
    if [ -e "$f" ]; then echo "$f $(stat -f '%i %m %z %Sp' "$f")"; else echo "$f absent"; fi
  done
  for f in /tmp/harpoon-serial.log "$PROD_SUPPORT/harpoon.log" "$PROD_SUPPORT/harpoon.log.1"; do
    if [ -e "$f" ]; then echo "$f $(stat -f '%i' "$f")"; else echo "$f absent"; fi
  done
}
BEFORE="$(fingerprint)"

TMP="$(mktemp -d)"
FAKE_PID=""; HOLD_PID=""
cleanup() {
  [ -z "$FAKE_PID" ] || kill "$FAKE_PID" 2>/dev/null || true
  [ -z "$HOLD_PID" ] || { kill "$HOLD_PID" 2>/dev/null; wait "$HOLD_PID" 2>/dev/null; } || true
  rm -rf "$TMP"
}
trap cleanup EXIT

ROOT="$TMP/ns"
export HARPOON_TEST_TMPDIR="$ROOT"
unset HARPOON_DOCKER_SOCK HARPOON_DISK HARPOON_TEST_MODE HARPOON_ALLOW_TMP_FALLBACK || true

# cli [env...] -- args  => OUT, CODE
cli() { OUT="$("$BIN" "$@" 2>&1)" && CODE=0 || CODE=$?; }

# --- 1. The namespace cannot be established unsafely: refuse before any command runs. ---------
refuses() { # name, then env assignments via `env`
  name="$1"; shift
  OUT="$(env "$@" "$BIN" status --json 2>&1)" && CODE=0 || CODE=$?
  [ "$CODE" -eq 2 ] || fail "$name: expected exit 2, got $CODE: $OUT"
  printf '%s' "$OUT" | grep -q "HARPOON_ISOLATION_FAILED" || fail "$name: no isolation error: $OUT"
}
refuses "relative root" HARPOON_TEST_TMPDIR=relative-dir
refuses "root is /tmp" HARPOON_TEST_TMPDIR=/tmp
ln -s /tmp "$TMP/tmp-link"
refuses "symlink to /tmp" HARPOON_TEST_TMPDIR="$TMP/tmp-link"
refuses "too long for a socket path" HARPOON_TEST_TMPDIR="$TMP/$(printf 'a%.0s' $(seq 1 100))"
refuses "docker socket override" HARPOON_DOCKER_SOCK=/tmp/harpoon-docker.sock
refuses "production disk" HARPOON_DISK="$PROD_SUPPORT/data/harpoon-root.img"
if [ -d "$PROD_SUPPORT" ]; then
  ln -s "$PROD_SUPPORT" "$TMP/support-link"
  refuses "symlink to production state" HARPOON_TEST_TMPDIR="$TMP/support-link"
fi
pass "unsafe namespaces are refused before any command runs"

# --- 2. Every lifecycle name resolves inside the root and none equals a production name. ------
cli status --json
[ "$CODE" -eq 0 ] || [ "$CODE" -eq 1 ] || fail "status --json exited $CODE: $OUT"
printf '%s' "$OUT" | python3 -I -c '
import json, os, sys
root = os.environ["HARPOON_TEST_TMPDIR"]
doc = json.load(sys.stdin)
keys = ["socketPath", "lockPath", "lifecycleLockPath", "controlSocketPath", "mgmtSocketPath", "stopFilePath", "logPath"]
assert doc.get("isolated") is True, "binary does not report an isolated namespace"
for k in keys:
    p = doc.get(k)
    assert p, "missing " + k
    assert p.startswith(root + "/"), "%s escapes the root: %s" % (k, p)
    assert not p.startswith("/tmp/"), "%s aliases /tmp: %s" % (k, p)
    assert "Library/Application Support/Harpoon" not in p, "%s aliases production state: %s" % (k, p)
' || fail "lifecycle paths are not isolated (is HARPOON_BIN a build from before the isolation change?)"
pass "locks, sockets, stop file and state resolve inside the test root"

# --- 3. A fake engine lives in the namespace; invalid restarts must not touch it. -------------
mkdir -p "$ROOT"
FAKE="$ROOT/harpoon-fake-engine"
cp /bin/sleep "$FAKE" && codesign --force -s - "$FAKE" >/dev/null 2>&1 || fail "cannot build fake engine"
# Started from a subshell so it is not our child: a stopped child stays a zombie until reaped and still looks alive.
FAKE_PID="$( ("$FAKE" 600 >/dev/null 2>&1 & echo $!) )"
printf '%s\n' "$FAKE_PID" > "$ROOT/runtime.pid"
python3 -I -c '
import fcntl, sys, time
h = open(sys.argv[1], "a+"); fcntl.flock(h, fcntl.LOCK_EX)
open(sys.argv[2], "w").write("held")
time.sleep(600)' "$ROOT/harpoon.lock" "$ROOT/lock-ready" & HOLD_PID=$!
for _ in $(seq 1 50); do [ -e "$ROOT/lock-ready" ] && break; sleep 0.1; done
[ -e "$ROOT/lock-ready" ] || fail "fake lock holder did not start"

engine_untouched() {
  kill -0 "$FAKE_PID" 2>/dev/null || fail "$1: the fake engine was stopped"
  kill -0 "$HOLD_PID" 2>/dev/null || fail "$1: the lock holder died"
  [ "$(cat "$ROOT/runtime.pid")" = "$FAKE_PID" ] || fail "$1: runtime.pid changed"
  [ ! -e "$ROOT/harpoon-stop" ] || fail "$1: a stop file was written"
  [ ! -e "$ROOT/harpoon.log.1" ] || fail "$1: the log was rotated (a start was attempted)"
}
engine_untouched "setup"

invalid_restart() {
  name="$1"; shift
  cli restart "$@"
  [ "$CODE" -ne 0 ] || fail "$name: restart unexpectedly succeeded: $OUT"
  printf '%s' "$OUT" | grep -q "restart refused before stopping anything" || fail "$name: not refused up front: $OUT"
  printf '%s' "$OUT" | grep -q "Stopping Harpoon" && fail "$name: reached the stop path: $OUT"
  engine_untouched "$name"
}
invalid_restart "memory not a number" --memory definitely-not-a-number
invalid_restart "memory below minimum" --memory 1
invalid_restart "cpus not a number" --cpus many
invalid_restart "cpus too many" --cpus 99
invalid_restart "dangling --memory" --memory
invalid_restart "kernel missing" --kernel "$TMP/no-such-kernel"
invalid_restart "initramfs missing" --initramfs "$TMP/no-such-initramfs"
invalid_restart "disk-size invalid" --disk-size nope
invalid_restart "disk-size too small" --disk-size 1M
# Deliberately no accepted-options case: it would stop the fake engine and boot a VM.
pass "invalid restart options fail before any stop; the running engine is untouched"

# --- 4. Control: the fake engine IS stoppable, so the checks above can fail. ------------------
cli stop
[ "$CODE" -eq 0 ] || fail "control stop failed: $OUT"
if kill -0 "$FAKE_PID" 2>/dev/null; then fail "control: stop did not stop the fake engine, the checks above prove nothing"; fi
FAKE_PID=""
pass "control: a stop in the namespace stops the fake engine"

# --- 5. The real engine's files are unchanged by all of the above. ----------------------------
AFTER="$(fingerprint)"
[ "$BEFORE" = "$AFTER" ] || { echo "$BEFORE" > "$TMP/before"; echo "$AFTER" > "$TMP/after"; diff "$TMP/before" "$TMP/after" >&2 || true; fail "production lifecycle files changed"; }
pass "production locks, sockets and state are identical before and after"

echo "LIFECYCLE_ISOLATION_PASS"
