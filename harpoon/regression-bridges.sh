#!/bin/sh
set -eu
# Regression for bridge lifecycle bug: sockets must remain after client disconnect, vanish only on STOPPING/FAILED
HOST_SOCK="/tmp/harpoon-docker.sock"
CTRL_SOCK="/tmp/harpoon-control"
say() { echo "[regression] $*"; }
fail() { echo "[regression] FAIL $*" >&2; exit 1; }
need_running() {
  [ -S "$HOST_SOCK" ] || fail "HARPOON not RUNNING: $HOST_SOCK missing"
  [ -S "$CTRL_SOCK" ] || fail "HARPOON not RUNNING: $CTRL_SOCK missing"
  ls -l "$HOST_SOCK" "$CTRL_SOCK" | grep -q "srw-------" || fail "perms not 0600"
  say "RUNNING sockets exist 0600"
}
say "=== regression: RUNNING -> client disconnect -> listener remains ==="
need_running
export DOCKER_HOST="unix://$HOST_SOCK"
say "single docker version (client connect/disconnect)"
DOCKER_HOST="unix://$HOST_SOCK" docker version 2>&1 | head -n 5
[ -S "$HOST_SOCK" ] || fail "after single client disconnect $HOST_SOCK disappeared (bug)"
[ -S "$CTRL_SOCK" ] || fail "after single client disconnect $CTRL_SOCK disappeared (bug)"
say "PASS single disconnect listener remains"
say "concurrent 5x docker run (vsock half-close, keep-alive) — bounded, captures output/exit per N"
rm -f /tmp/harpoon-regression-concurrent-*.log 2>&1 | true
pids=""
for i in 1 2 3 4 5; do
  (
    set +e
    out=$(DOCKER_HOST="unix://$HOST_SOCK" docker run --rm alpine:3.22 echo "hi $i" 2>&1)
    status=$?
    printf "%s" "$out" > "/tmp/harpoon-regression-concurrent-$i.log"
    printf "%d" "$status" > "/tmp/harpoon-regression-concurrent-$i.status"
    if [ $status -ne 0 ]; then
      echo "[regression] concurrent $i exit $status output: $out" >&2
      exit 1
    fi
    echo "$out" | grep -q "hi $i" || { echo "[regression] concurrent $i missing hi $i, output: $out" >&2; exit 1; }
  ) &
  pids="$pids $!"
done
# wait for all five and capture any failure
failed=0
for pid in $pids; do
  if ! wait "$pid"; then failed=1; fi
done
if [ $failed -ne 0 ]; then
  say "concurrent outputs:"
  for i in 1 2 3 4 5; do echo "--- $i status $(cat /tmp/harpoon-regression-concurrent-$i.status 2>&1) ---"; cat "/tmp/harpoon-regression-concurrent-$i.log" 2>&1; done
  fail "concurrent 5x failed (see outputs above)"
fi
# verify outputs after wait (defensive, also proves exact hi N)
for i in 1 2 3 4 5; do
  out=$(cat "/tmp/harpoon-regression-concurrent-$i.log" 2>&1)
  echo "$out" | grep -q "hi $i" || fail "concurrent $i final check missing hi $i: $out"
done
rm -f /tmp/harpoon-regression-concurrent-*.log /tmp/harpoon-regression-concurrent-*.status 2>&1 | true
[ -S "$HOST_SOCK" ] || fail "after 5 concurrent disconnects $HOST_SOCK gone"
[ -S "$CTRL_SOCK" ] || fail "after 5 concurrent $CTRL_SOCK gone"
say "PASS 5 concurrent disconnects listener remains"
say "multiple sequential docker ps/images/info"
for n in 1 2 3; do DOCKER_HOST="unix://$HOST_SOCK" docker ps >/dev/null; DOCKER_HOST="unix://$HOST_SOCK" docker images >/dev/null; DOCKER_HOST="unix://$HOST_SOCK" docker info >/dev/null 2>&1 || true; done
[ -S "$HOST_SOCK" ] || fail "after sequential ps/images/info $HOST_SOCK gone"
say "PASS sequential listener remains"
say "STOPPING -> sockets must disappear after SIGTERM"
# This part is manual: caller should SIGTERM harpoon and check
say "regression RUNNING checks PASS (STOPPING/FAILED checks require SIGTERM/failed artifact — see harpoon/README)"
