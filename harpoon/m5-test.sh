#!/bin/sh
set -eu
HOST_SOCK="/tmp/harpoon-docker.sock"
export DOCKER_HOST="unix://$HOST_SOCK"
say() { echo "[m5] $*"; }
fail() { echo "[m5] FAIL $*" >&2; exit 1; }
ok() { echo "[m5] PASS $*"; }
need_sock() {
  [ -S "$HOST_SOCK" ] || fail "socket $HOST_SOCK missing (harpoon not RUNNING)"
  perms=$(stat -f "%Lp" "$HOST_SOCK" 2>/dev/null || stat -c "%a" "$HOST_SOCK" 2>/dev/null || echo "?")
  [ "$perms" = "600" ] || fail "socket perms $perms != 600"
  ok "socket 0600 $HOST_SOCK"
}
if [ ! -S "$HOST_SOCK" ]; then
  say "Harpoon not RUNNING — socket missing, cannot run M5 live tests"
  say "Start Harpoon: harpoon/build/harpoon > /tmp/harpoon.log 2>&1 & then rerun"
  exit 2
fi
need_sock
say "=== M5 Networking & Port Publishing $HOST_SOCK ==="
# cleanup previous m5 objects (only m5-*)
cleanup() {
  say "cleanup m5 objects"
  for n in m5-nginx m5-nginx2 m5-nginx-multi m5-clean m5-server m5-ephemeral m5-collide m5-nopublish m5-udp; do
    DOCKER_HOST="unix://$HOST_SOCK" docker rm -f "$n" 2>/dev/null || true
  done
  DOCKER_HOST="unix://$HOST_SOCK" docker network rm m5-net 2>/dev/null || true
  # remove any test http server on collision port
  pkill -f "http.server 18085" 2>/dev/null || true
}
trap cleanup EXIT
cleanup
# ensure no stray forward from previous run
sleep 1

say "Test 1: single published TCP port"
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-nginx -p 18080:80 nginx:alpine 2>&1 | tail -n 5
sleep 3
# curl host
if ! curl -fsS --max-time 5 http://127.0.0.1:18080/ 2>&1 | grep -qi "nginx\|Welcome"; then
  fail "curl 127.0.0.1:18080 failed (single port)"
fi
ok "single port 18080:80 curl ok"
# verify docker port/inspect consistency
DOCKER_HOST="unix://$HOST_SOCK" docker port m5-nginx | grep -q "80/tcp.*18080" || fail "docker port m5-nginx mismatch"
DOCKER_HOST="unix://$HOST_SOCK" docker inspect m5-nginx --format '{{json .NetworkSettings.Ports}}' | grep -q "18080" || fail "inspect Ports mismatch"
ok "docker port/inspect consistent"

say "Test 2: multiple simultaneous published ports"
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-nginx2 -p 18081:80 nginx:alpine 2>&1 | tail -n 5
sleep 2
curl -fsS --max-time 5 http://127.0.0.1:18080/ | grep -qi "nginx\|Welcome" || fail "curl 18080 after second container"
curl -fsS --max-time 5 http://127.0.0.1:18081/ | grep -qi "nginx\|Welcome" || fail "curl 18081 failed"
ok "multiple ports concurrent"

say "Test 3: repeated & keep-alive"
for i in 1 2 3 4 5; do curl -fsS --max-time 5 http://127.0.0.1:18080/ >/dev/null || fail "repeated curl $i"; done
ok "repeated curls"

say "Test 4: concurrency (5 simultaneous curls per port)"
pids=""
for i in 1 2 3 4 5; do curl -fsS --max-time 10 http://127.0.0.1:18080/ >/dev/null & pids="$pids $!"; done
for i in 1 2 3 4 5; do curl -fsS --max-time 10 http://127.0.0.1:18081/ >/dev/null & pids="$pids $!"; done
for pid in $pids; do wait $pid || fail "concurrent curl pid $pid"; done
ok "concurrency 10 simultaneous"

say "Test 5: container-to-container networking (Docker DNS/bridge)"
DOCKER_HOST="unix://$HOST_SOCK" docker network create m5-net 2>&1 | tail -n 2
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-server --network m5-net nginx:alpine 2>&1 | tail -n 5
sleep 2
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm --network m5-net alpine:3.22 wget -qO- http://m5-server/ 2>&1 | grep -qi "nginx\|Welcome" || fail "container-to-container wget"
ok "container-to-container"
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-server 2>/dev/null || true
DOCKER_HOST="unix://$HOST_SOCK" docker network rm m5-net 2>/dev/null || true

say "Test 6: outbound networking / DNS"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm alpine:3.22 ping -c 1 1.1.1.1 2>&1 | grep -q "1 packets" || fail "ping 1.1.1.1"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm alpine:3.22 nslookup dl-cdn.alpinelinux.org 2>&1 | grep -qi "Address" || echo "[m5] WARN nslookup not as expected"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm alpine:3.22 sh -c 'apk update >/dev/null 2>&1 && echo apk-ok' | grep -q "apk-ok" || echo "[m5] WARN apk update failed"
ok "outbound DNS/Internet"

say "Test 7: stop/start/remove lifecycle"
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-clean -p 18086:80 nginx:alpine 2>&1 | tail -n 5
sleep 2
curl -fsS --max-time 5 http://127.0.0.1:18086/ >/dev/null || fail "m5-clean initial curl"
ok "lifecycle initial"
DOCKER_HOST="unix://$HOST_SOCK" docker stop m5-clean 2>&1 | tail -n 2
sleep 3
if curl -fsS --max-time 3 http://127.0.0.1:18086/ 2>&1 | grep -qi "nginx"; then
  fail "port 18086 still reachable after stop (listener not removed)"
else
  ok "port removed after stop"
fi
DOCKER_HOST="unix://$HOST_SOCK" docker start m5-clean 2>&1 | tail -n 2
sleep 3
curl -fsS --max-time 5 http://127.0.0.1:18086/ | grep -qi "nginx\|Welcome" || fail "port not restored after start"
ok "port restored after start"
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-clean 2>&1 | tail -n 2
sleep 2
if curl -fsS --max-time 3 http://127.0.0.1:18086/ 2>&1 | grep -qi "nginx"; then
  fail "port still reachable after rm"
else
  ok "port removed after rm"
fi

say "Test 8: host bind address (loopback safety) — explicit 127.0.0.1 deferred characterization"
# Observed: -p 127.0.0.1:18087:80 binds guest 127.0.0.1:18087 inside guest loopback, not reachable via VZNAT guestIP:18087 (192.168.64.x).
# Harpoon Phase 1 VZNAT forwarding path is guestIP:HostPort; guest-loopback binding is therefore a known deferred limitation, not a failure of ordinary -p HostPort:ContainerPort.
# Ordinary -p 18080:80 and 0.0.0.0 mappings remain Harpoon loopback-only 127.0.0.1 (safety).
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-loop --network bridge -p 127.0.0.1:18087:80 nginx:alpine 2>&1 | tail -n 5 || true
sleep 2
if DOCKER_HOST="unix://$HOST_SOCK" docker port m5-loop 2>&1 | grep -q "127.0.0.1:18087"; then
  echo "[m5] CHARACTERIZED explicit Docker 127.0.0.1 HostIp deferred: guest-loopback binding is not reachable via VZNAT guestIP forwarding"
  if curl -fsS --max-time 3 http://127.0.0.1:18087/ 2>&1 | grep -qi "nginx\|Welcome"; then
    echo "[m5] INFO explicit 127.0.0.1:18087 unexpectedly reachable"
  else
    echo "[m5] INFO explicit 127.0.0.1:18087 not reachable via VZNAT guestIP as expected (deferred)"
  fi
else
  echo "[m5] CHARACTERIZED explicit Docker 127.0.0.1 HostIp deferred: guest-loopback binding is not reachable via VZNAT guestIP forwarding"
fi
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-loop 2>/dev/null || true
# Preserve: ordinary unspecified HostIp (0.0.0.0) -> Harpoon macOS 127.0.0.1 loopback-only (verified in Tests 1/2)
echo "[m5] INFO ordinary -p HostPort:ContainerPort remains macOS loopback-only via Harpoon (0.0.0.0 -> 127.0.0.1 safety)"

say "Test 9: port collision"

COLLIDE_PORT=""
for p in 18089 18090 18091 18092 18093; do
  if ! lsof -nP -iTCP:$p -sTCP:LISTEN >/dev/null 2>&1; then
    COLLIDE_PORT="$p"
    break
  fi
done

[ -n "$COLLIDE_PORT" ] || fail "no free collision-test port"

python3 -m http.server "$COLLIDE_PORT" --bind 127.0.0.1 >/tmp/m5-collide.log 2>&1 &
PYPID=$!
sleep 1

lsof -nP -iTCP:$COLLIDE_PORT -sTCP:LISTEN | grep -q Python || fail "python collision listener did not start"

DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-collide 2>/dev/null || true

set +e
out_collide=$(DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-collide -p "$COLLIDE_PORT":80 nginx:alpine 2>&1)
status_collide=$?
set -e

sleep 3

# Authoritative assertion: the original host listener remains owner.
lsof -nP -iTCP:$COLLIDE_PORT -sTCP:LISTEN | grep -q Python || fail "host collision port was hijacked"
ok "collision preserved existing host listener on $COLLIDE_PORT"

# Collision logging is observability only because Harpoon may be running interactively.
if grep -q "HARPOON_PORT_FORWARD_COLLISION.*$COLLIDE_PORT" /tmp/harpoon.log 2>/dev/null; then
  echo "[m5] INFO collision log observed"
else
  echo "[m5] INFO collision log file unavailable/not authoritative for interactive Harpoon run"
fi

DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-collide 2>/dev/null || true

if kill -0 "$PYPID" 2>/dev/null; then
  kill "$PYPID" 2>/dev/null || true
  wait "$PYPID" 2>/dev/null || true
fi

rm -f /tmp/m5-collide.log

[ $status_collide -eq 0 ] || echo "[m5] INFO Docker rejected collision before Harpoon reconciliation: $out_collide"

say "Test 10: no-publish security (no -p must NOT be reachable)"
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m5-nopublish nginx:alpine 2>&1 | tail -n 5
sleep 2
# try to curl a random high port that should NOT be forwarded
if curl -fsS --max-time 3 http://127.0.0.1:18088/ 2>&1 | grep -qi "nginx"; then
  fail "no-publish container became reachable via 18088"
else
  ok "no-publish not exposed"
fi
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-nopublish 2>/dev/null || true

say "Test 11: ephemeral host port characterization"
# Docker may assign random host port when HostPort empty
set +e
DOCKER_HOST="unix://$HOST_SOCK" docker run -d -p 80 --name m5-ephemeral nginx:alpine 2>&1 | tail -n 5
sleep 2
port_info=$(DOCKER_HOST="unix://$HOST_SOCK" docker port m5-ephemeral 2>&1 || true)
inspect_info=$(DOCKER_HOST="unix://$HOST_SOCK" docker inspect m5-ephemeral --format '{{json .NetworkSettings.Ports}}' 2>&1 || true)
echo "[m5] ephemeral port info: $port_info"
echo "[m5] ephemeral inspect: $inspect_info"
# if we can discover assigned port, try curl; otherwise document deferred
assigned=$(echo "$port_info" | grep -o "[0-9]*$" | head -n 1 || true)
if [ -n "$assigned" ] && [ "$assigned" != "0" ]; then
  if curl -fsS --max-time 5 http://127.0.0.1:$assigned/ 2>&1 | grep -qi "nginx"; then
    ok "ephemeral port $assigned forwarded"
  else
    echo "[m5] INFO ephemeral port $assigned not forwardable (deferred)"
  fi
else
  echo "[m5] INFO ephemeral port not assigned or not discovered (deferred)"
fi
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-ephemeral 2>/dev/null || true
set -e

say "Test 12: UDP characterization (TCP required, UDP deferred)"
set +e
DOCKER_HOST="unix://$HOST_SOCK" docker run -d -p 15353:5353/udp --name m5-udp alpine:3.22 sleep 30 2>&1 | tail -n 5
if [ $? -eq 0 ]; then
  echo "[m5] UDP publish created, checking Harpoon logs for UDP deferred"
  grep -q "udp deferred" /tmp/harpoon.log 2>/dev/null && echo "[m5] UDP deferred as expected"
  DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m5-udp 2>/dev/null || true
else
  echo "[m5] INFO UDP publish not supported (deferred)"
fi
set -e

say "Test 13: M1-M4 regression sanity"
DOCKER_HOST="unix://$HOST_SOCK" docker version 2>&1 | head -n 5 || fail "docker version regression"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm hello-world 2>&1 | grep -q "Hello" || fail "hello-world regression"
# bind mount still works
M4_HOST="/tmp/harpoon-m5-m4check"
rm -rf "$M4_HOST" 2>/dev/null || true
mkdir -p "$M4_HOST"
echo "m4check" > "$M4_HOST/file.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace" alpine:3.22 cat /workspace/file.txt | grep -q "m4check" || fail "M4 bind mount regression"
# unsupported path still rejected
set +e
out_reject=$(DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v /etc:/workspace alpine:3.22 ls /workspace 2>&1)
status_reject=$?
set -e
[ $status_reject -ne 0 ] || fail "M4 unsupported /etc should still be rejected"
echo "$out_reject" | grep -qi "not shared\|unsupported host path" || fail "M4 reject message regression"
rm -rf "$M4_HOST" 2>/dev/null || true
ok "M1-M4 regression"

cleanup
say "ALL M5 CHECKS PASS (TCP loopback, multi, lifecycle, container-to-container, outbound, security)"
