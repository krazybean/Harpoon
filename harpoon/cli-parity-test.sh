#!/bin/sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="${HARPOON_BIN:-$SCRIPT_DIR/build/harpoon}"

if [ ! -x "$BIN" ]; then
  echo "FAIL: Harpoon binary not found/executable: $BIN" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# This suite issues real lifecycle verbs (start/stop/restart). They must only ever see a private
# namespace: HARPOON_TEST_TMPDIR moves the locks, sockets, stop file, state and disk under $TMP, so a
# Harpoon that is serving on this machine is never named. If the binary cannot prove that (an older
# build, a root that aliases /tmp), refuse before running a single lifecycle command.
export HARPOON_TEST_TMPDIR="$TMP/ns"
unset HARPOON_DOCKER_SOCK HARPOON_DISK HARPOON_TEST_MODE HARPOON_ALLOW_TMP_FALLBACK || true
ISOLATION="$("$BIN" status --json 2>&1)" || true
printf '%s' "$ISOLATION" | python3 -I -c '
import json, os, sys
root = os.environ["HARPOON_TEST_TMPDIR"] + "/"
doc = json.load(sys.stdin)
keys = ["socketPath", "lockPath", "lifecycleLockPath", "controlSocketPath", "mgmtSocketPath", "stopFilePath", "logPath"]
assert doc.get("isolated") is True and all(str(doc.get(k, "")).startswith(root) for k in keys)
' 2>/dev/null || {
  echo "FAIL: cannot establish an isolated Harpoon namespace with $BIN; refusing to run lifecycle commands" >&2
  echo "      (rebuild with harpoon/build.sh; do not point HARPOON_BIN at an installed or running build)" >&2
  exit 1
}
ENDPOINT="unix://$HARPOON_TEST_TMPDIR/harpoon-docker.sock"

FAKE_DOCKER="$TMP/docker"
FAKE_LOG="$TMP/docker-args.log"

cat > "$FAKE_DOCKER" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "${HARPOON_FAKE_DOCKER_LOG:?}"
exit 0
EOF
chmod +x "$FAKE_DOCKER"
export HARPOON_DOCKER_CLI="$FAKE_DOCKER"
export HARPOON_FAKE_DOCKER_LOG="$FAKE_LOG"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

expect_forward() {
  expected="$1"
  shift
  : > "$FAKE_LOG"
  "$BIN" "$@" >/dev/null 2>&1 || fail "command failed: harpoon $*"
  actual="$(tail -n 1 "$FAKE_LOG" 2>/dev/null || true)"
  [ "$actual" = "$expected" ] || fail "harpoon $* forwarded '$actual', expected '$expected'"
}

# Help exposes the new namespace without removing the historical help body.
"$BIN" help 2>&1 | grep -q "harpoon machine" || fail "main help missing machine namespace"
"$BIN" machine --help 2>&1 | grep -q "Machine lifecycle" || fail "machine help missing"
"$BIN" start --help 2>&1 | grep -q "usage: harpoon start" || fail "start help regression"
"$BIN" logs --help 2>&1 | grep -q "usage: harpoon logs" || fail "logs help regression"
"$BIN" config --help 2>&1 | grep -q "usage: harpoon config" || fail "config help regression"
"$BIN" docker --help 2>&1 | grep -q "usage: harpoon docker" || fail "docker help regression"
"$BIN" status --help 2>&1 | grep -q "usage: harpoon status" || fail "status help regression"
"$BIN" disk --help 2>&1 | grep -q "usage: harpoon disk" || fail "disk help regression"
pass "help and machine namespace"

# Common Docker/Podman verbs are pinned directly to Harpoon's engine socket.
expect_forward "--host $ENDPOINT ps -a" ps -a
expect_forward "--host $ENDPOINT images" images
expect_forward "--host $ENDPOINT pull alpine:3.22" pull alpine:3.22
expect_forward "--host $ENDPOINT build -t parity-test ." build -t parity-test .
expect_forward "--host $ENDPOINT login -u alice registry.example.com" login -u alice registry.example.com
expect_forward "--host $ENDPOINT logout registry.example.com" logout registry.example.com
expect_forward "--host $ENDPOINT volume ls" volume ls
expect_forward "--host $ENDPOINT network ls" network ls
expect_forward "--host $ENDPOINT system df" system df
expect_forward "--host $ENDPOINT compose version" compose version
pass "non-colliding container commands"

# Colliding verbs switch to container semantics when a target/image is supplied.
expect_forward "--host $ENDPOINT run --rm alpine:3.22 echo ok" run --rm alpine:3.22 echo ok
expect_forward "--host $ENDPOINT start web" start web
expect_forward "--host $ENDPOINT stop web" stop web
expect_forward "--host $ENDPOINT restart web" restart web
expect_forward "--host $ENDPOINT logs web" logs web
expect_forward "--host $ENDPOINT exec web sh -lc true" exec web sh -lc true
pass "collision-aware container commands"

# Historical lifecycle spellings remain machine aliases and must not hit Docker.
: > "$FAKE_LOG"
"$BIN" start --help >/dev/null 2>&1 || fail "legacy start --help failed"
[ ! -s "$FAKE_LOG" ] || fail "legacy start --help unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" start --cpus definitely-not-a-number >/dev/null 2>&1 || true
[ ! -s "$FAKE_LOG" ] || fail "legacy start --cpus unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" restart --memory definitely-not-a-number >/dev/null 2>&1 || true
[ ! -s "$FAKE_LOG" ] || fail "legacy restart --memory unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" stop --help >/dev/null 2>&1 || fail "legacy stop --help failed"
[ ! -s "$FAKE_LOG" ] || fail "legacy stop --help unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" run --help >/dev/null 2>&1 || fail "legacy run --help failed"
[ ! -s "$FAKE_LOG" ] || fail "legacy run --help unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" logs --path >/dev/null 2>&1 || fail "legacy logs --path failed"
[ ! -s "$FAKE_LOG" ] || fail "legacy logs --path unexpectedly forwarded to Docker"

: > "$FAKE_LOG"
"$BIN" stop >/dev/null 2>&1 || fail "legacy bare stop failed"
[ ! -s "$FAKE_LOG" ] || fail "legacy bare stop unexpectedly forwarded to Docker"

"$BIN" machine status >/dev/null 2>&1 || fail "machine status failed"
"$BIN" status >/dev/null 2>&1 || fail "legacy status alias failed"
pass "legacy lifecycle aliases"

echo "CLI_PARITY_TEST_PASS"
