#!/bin/bash
. "$(dirname "$0")/../tools/vm-guard-reexec.sh"
# Published-port forwarding, end to end: explicit guest-loopback publishing (-p 127.0.0.1:P:C) and ordinary
# publishing (-p P:C) must both be reachable from Mac loopback, and only from there.
#
# LIVE test: it boots containers inside the engine it is pointed at, so it REFUSES to run unless that engine is
# an isolated test namespace (HARPOON_TEST_TMPDIR, see HarpoonRoots) — it can never touch a serving engine.
#   export HARPOON_TEST_TMPDIR=/tmp/hpft            # short path (unix socket limit)
#   harpoon-under-test start --cpus 1 --memory 1024 --disk-size 8G
#   export DOCKER_HOST=unix://$HARPOON_TEST_TMPDIR/harpoon-docker.sock
#   HARPOON_BIN=harpoon-under-test sh harpoon/regression-loopback-publish.sh
set -u
BIN="${HARPOON_BIN:?set HARPOON_BIN to the harpoon binary under test}"
ROOT="${HARPOON_TEST_TMPDIR:-}"
IMAGE="${PUBLISH_TEST_IMAGE:-alpine:3.22}"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }
[ -n "$ROOT" ] || fail "refusing to run: HARPOON_TEST_TMPDIR is not set (this test must only target an isolated engine)"
[ "${DOCKER_HOST:-}" = "unix://$ROOT/harpoon-docker.sock" ] || fail "refusing to run: DOCKER_HOST must be unix://$ROOT/harpoon-docker.sock"
LOG="$ROOT/harpoon.log"
[ -f "$LOG" ] || fail "no engine log at $LOG (is the isolated engine running?)"
ENGINE_PID="$("$BIN" status | awk '/^PID:/{print $2; exit}')"
[ -n "$ENGINE_PID" ] || fail "isolated engine is not running"

PREFIX="hpub$$"
cleanup() { docker rm -f $(docker ps -aq --filter "name=$PREFIX") >/dev/null 2>&1 || true; [ -n "${HOLD_PID:-}" ] && kill "$HOLD_PID" 2>/dev/null; true; }
trap cleanup EXIT

free_port() { local p; while :; do p=$((41000 + RANDOM % 1000)); lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1 || { echo $p; return; }; done; }
# alpine's busybox has nc but no httpd: a persistent nc -lk server answering GET /big (4 MiB, random) and anything else ("harpoon-ok").
# The handler travels in an environment variable so no quoting layer touches it.
SERVER_SH='read -r line
while read -r h; do [ "$h" = "$(printf "\r")" ] && break; done
case "$line" in *" /big "*) f=/w/big ;; *) f=/w/index.html ;; esac
printf "HTTP/1.0 200 OK\r\nContent-Length: %s\r\nConnection: close\r\n\r\n" "$(wc -c < "$f")"
cat "$f"'
run_server() { # name publish-spec
  docker run -d --name "$1" -e SRV="#!/bin/sh
$SERVER_SH" -p "$2:8080" "$IMAGE" sh -c 'mkdir -p /w && echo harpoon-ok > /w/index.html && dd if=/dev/urandom of=/w/big bs=1k count=4096 2>/dev/null && printf "%s\n" "$SRV" > /srv.sh && chmod +x /srv.sh && exec nc -lk -p 8080 -e /srv.sh' >/dev/null || fail "docker run $2"
}
wait_for() { # description command...  (forwarder sync is up to ~10 s; allow 40)
  local d="$1"; shift; local i; for i in $(seq 1 40); do "$@" >/dev/null 2>&1 && return 0; sleep 1; done; fail "timed out: $d"
}
body_is_ok() { [ "$(curl -fsS --max-time 3 "http://127.0.0.1:$1/index.html" 2>/dev/null)" = "harpoon-ok" ]; }
relay_gone() { [ "$(relay_count "$1")" = "0" ]; }
listener_gone() { ! lsof -nP -iTCP:$1 -sTCP:LISTEN >/dev/null 2>&1; }
relay_count() { "$BIN" exec -- sh -c "pgrep -f '[V]SOCK-LISTEN:$((65536 + $1)),' | wc -l" | tr -d ' \r\n'; }
listeners_of() { lsof -nP -a -p "$ENGINE_PID" -iTCP:$1 -sTCP:LISTEN -Fn 2>/dev/null | grep '^n' | sort -u; }

echo "== explicit loopback: -p 127.0.0.1:P:8080"
LP=$(free_port); run_server "$PREFIX-loop" "127.0.0.1:$LP"
wait_for "loopback-published port answers on Mac 127.0.0.1:$LP" body_is_ok "$LP"
pass "explicit loopback publish reachable from Mac loopback"
GUEST_SUM=$(docker exec "$PREFIX-loop" sha256sum /w/big | awk '{print $1}')
HOST_SUM=$(curl -fsS --max-time 30 "http://127.0.0.1:$LP/big" | shasum -a 256 | awk '{print $1}')
[ "$GUEST_SUM" = "$HOST_SUM" ] || fail "4 MiB body differs through the relay ($GUEST_SUM vs $HOST_SUM)"
pass "4 MiB response is byte-identical through the vsock relay"
burst() { # port count: every connection in a simultaneous burst must succeed
  local pids="" bad=0 x; for i in $(seq 1 "$2"); do curl -fsS --max-time 10 "http://127.0.0.1:$1/index.html" -o /dev/null & pids="$pids $!"; done
  for x in $pids; do wait "$x" || bad=$((bad + 1)); done; [ "$bad" = 0 ] || fail "$bad of $2 simultaneous connections to port $1 failed"
}
for round in 1 2 3 4 5; do burst "$LP" 16; done
pass "5 bursts of 16 simultaneous connections through the relay, none reset"
[ "$(relay_count "$LP")" -ge 1 ] || fail "no guest relay for $LP"
pass "guest relay running for $LP"

echo "== ordinary publishing: -p P:8080 (the form deployments use: no HostIp)"
OP=$(free_port); run_server "$PREFIX-plain" "$OP"
wait_for "ordinary published port answers on Mac 127.0.0.1:$OP" body_is_ok "$OP"
pass "ordinary publish reachable from Mac loopback"
[ "$(relay_count "$OP")" = "0" ] || fail "ordinary publish must not start a guest relay"
pass "ordinary publish uses no guest relay (path unchanged)"

echo "== host exposure: Mac loopback only"
for p in "$LP" "$OP"; do
  [ "$(listeners_of "$p")" = "n127.0.0.1:$p" ] || fail "port $p listeners are not exactly 127.0.0.1:$p: $(listeners_of "$p")"
  LAN="$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || true)"
  if [ -n "$LAN" ]; then curl -s --max-time 3 -o /dev/null "http://$LAN:$p/" && fail "port $p answers on LAN address $LAN"; fi
done
pass "both listeners are exactly 127.0.0.1:<port> owned by the engine; not reachable on the LAN address"

echo "== port conflict: host port already held by another process"
CP=$(free_port)
# a foreign process that keeps the port bound without ever answering (nc would exit after one probe and free it)
python3 -c 'import socket,sys,time; s=socket.socket(); s.bind(("127.0.0.1",int(sys.argv[1]))); s.listen(5); time.sleep(600)' "$CP" & HOLD_PID=$!
sleep 1
run_server "$PREFIX-conflict" "127.0.0.1:$CP"
wait_for "collision logged" grep -q "HARPOON_PORT_FORWARD_COLLISION $PREFIX-conflict" "$LOG"
[ "$(curl -s --max-time 3 "http://127.0.0.1:$CP/index.html" 2>/dev/null)" = "harpoon-ok" ] && fail "conflicting port served the container (another process owns it)"
sleep 12
[ "$(relay_count "$CP")" = "0" ] || fail "guest relay left behind while the host port is not ours"
pass "collision reported; foreign listener not displaced; no stray guest relay"
kill "$HOLD_PID" 2>/dev/null; wait "$HOLD_PID" 2>/dev/null; HOLD_PID=""
wait_for "forward recovers once the port is free" body_is_ok "$CP"
pass "forward recovers at the next sync after the conflict clears"

echo "== listener cleanup: stop and remove"
docker stop "$PREFIX-loop" >/dev/null
wait_for "listener removed after docker stop (loopback)" listener_gone "$LP"
wait_for "guest relay removed after docker stop" relay_gone "$LP"
pass "stopped container: host listener and guest relay removed"
docker rm -f "$PREFIX-plain" "$PREFIX-conflict" >/dev/null
wait_for "listener removed after docker rm (ordinary)" listener_gone "$OP"
wait_for "listener removed after docker rm (conflict port)" listener_gone "$CP"
wait_for "guest relay removed after docker rm" relay_gone "$CP"
pass "removed containers: host listeners and guest relays removed"

echo "== port reuse: same port, other publish form, no stale state"
RP=$(free_port)
run_server "$PREFIX-r1" "127.0.0.1:$RP"; wait_for "r1 answers" body_is_ok "$RP"
docker rm -f "$PREFIX-r1" >/dev/null; run_server "$PREFIX-r2" "$RP"
wait_for "r2 (ordinary form) answers on the reused port" body_is_ok "$RP"
wait_for "loopback relay cleaned when the port was reused for an ordinary publish" relay_gone "$RP"
docker rm -f "$PREFIX-r2" >/dev/null; run_server "$PREFIX-r3" "127.0.0.1:$RP"
wait_for "r3 (loopback form) answers on the reused port" body_is_ok "$RP"
pass "port reused loopback -> ordinary -> loopback without stale routing"

echo "published-port forwarding live regression PASS"
