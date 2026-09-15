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
pass "help and machine namespace"

# Common Docker/Podman verbs are pinned directly to Harpoon's engine socket.
expect_forward "--host unix:///tmp/harpoon-docker.sock ps -a" ps -a
expect_forward "--host unix:///tmp/harpoon-docker.sock images" images
expect_forward "--host unix:///tmp/harpoon-docker.sock pull alpine:3.22" pull alpine:3.22
expect_forward "--host unix:///tmp/harpoon-docker.sock build -t parity-test ." build -t parity-test .
expect_forward "--host unix:///tmp/harpoon-docker.sock login -u alice registry.example.com" login -u alice registry.example.com
expect_forward "--host unix:///tmp/harpoon-docker.sock logout registry.example.com" logout registry.example.com
expect_forward "--host unix:///tmp/harpoon-docker.sock volume ls" volume ls
expect_forward "--host unix:///tmp/harpoon-docker.sock network ls" network ls
expect_forward "--host unix:///tmp/harpoon-docker.sock system df" system df
expect_forward "--host unix:///tmp/harpoon-docker.sock compose version" compose version
pass "non-colliding container commands"

# Colliding verbs switch to container semantics when a target/image is supplied.
expect_forward "--host unix:///tmp/harpoon-docker.sock run --rm alpine:3.22 echo ok" run --rm alpine:3.22 echo ok
expect_forward "--host unix:///tmp/harpoon-docker.sock start web" start web
expect_forward "--host unix:///tmp/harpoon-docker.sock stop web" stop web
expect_forward "--host unix:///tmp/harpoon-docker.sock restart web" restart web
expect_forward "--host unix:///tmp/harpoon-docker.sock logs web" logs web
expect_forward "--host unix:///tmp/harpoon-docker.sock exec web sh -lc true" exec web sh -lc true
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
