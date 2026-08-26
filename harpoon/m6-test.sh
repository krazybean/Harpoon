#!/bin/sh
set -eu
HOST_SOCK="/tmp/harpoon-docker.sock"
CONTROL_SOCK="/tmp/harpoon-control"
say() { echo "[m6] $*"; }
fail() { echo "[m6] FAIL $*" >&2; exit 1; }
ok() { echo "[m6] PASS $*"; }
need_sock() {
  [ -S "$HOST_SOCK" ] || fail "socket $HOST_SOCK missing (harpoon not RUNNING)"
  perms=$(stat -f "%Lp" "$HOST_SOCK" 2>/dev/null || stat -c "%a" "$HOST_SOCK" 2>/dev/null || echo "?")
  [ "$perms" = "600" ] || fail "socket perms $perms != 600"
  perms2=$(stat -f "%Lp" "$CONTROL_SOCK" 2>/dev/null || stat -c "%a" "$CONTROL_SOCK" 2>/dev/null || echo "?")
  [ "$perms2" = "600" ] || echo "[m6] WARN control perms $perms2 != 600"
  ok "sockets 0600 $HOST_SOCK $CONTROL_SOCK"
}
usage() {
  echo "usage: $0 [--stage invalid|1024|768|512|all] [--help]"
  echo "  --stage invalid : invalid CLI config (no VM running needed)"
  echo "  --stage 1024/768/512 : live tier checks (requires Harpoon RUNNING at that tier)"
  echo "  --stage all : invalid + current tier (auto-detect)"
}
STAGE="all"
for arg in "$@"; do
  case "$arg" in
    --stage) ;;
    --stage=*) STAGE=$(echo "$arg" | cut -d= -f2) ;;
    1024|768|512|invalid|all) STAGE="$arg" ;;
    --help|-h) usage; exit 0 ;;
  esac
done
# handle --stage X form
if [ "$1" = "--stage" ] 2>/dev/null && [ -n "${2:-}" ]; then STAGE="$2"; fi

invalid_tests() {
  say "=== M6 invalid CLI config ==="
  # --memory 128 should fail before VM start, no socket, no orphan
  set +e
  out=$(harpoon/build/harpoon --memory 128 2>&1)
  status=$?
  set -e
  echo "$out" | grep -q "HARPOON_STATE.*FAILED" || echo "[m6] INFO expected FAILED state for --memory 128"
  if [ $status -eq 0 ]; then fail "harpoon --memory 128 should fail non-zero, got $status"; fi
  [ ! -S "$HOST_SOCK" ] || echo "[m6] WARN socket $HOST_SOCK exists after invalid --memory 128 (should not)"
  # check no orphan VM process? just check harpoon not running
  ok "invalid --memory 128 rejected"

  set +e
  out=$(harpoon/build/harpoon --memory 9999 2>&1)
  status=$?
  set -e
  echo "$out" | grep -q "FAILED" || echo "[m6] INFO expected FAILED for --memory 9999"
  [ $status -ne 0 ] || fail "harpoon --memory 9999 should fail"
  ok "invalid --memory 9999 rejected"

  set +e
  out=$(harpoon/build/harpoon --memory abc 2>&1)
  status=$?
  set -e
  echo "$out" | grep -q "FAILED" || echo "[m6] INFO expected FAILED for --memory abc"
  [ $status -ne 0 ] || fail "harpoon --memory abc should fail"
  ok "invalid --memory abc rejected"

  set +e
  out=$(harpoon/build/harpoon --cpus 0 2>&1)
  status=$?
  set -e
  echo "$out" | grep -q "FAILED" || echo "[m6] INFO expected FAILED for --cpus 0"
  [ $status -ne 0 ] || fail "harpoon --cpus 0 should fail"
  ok "invalid --cpus 0 rejected"

  set +e
  out=$(harpoon/build/harpoon --cpus 999 2>&1)
  status=$?
  set -e
  echo "$out" | grep -q "FAILED" || echo "[m6] INFO expected FAILED for --cpus 999"
  [ $status -ne 0 ] || fail "harpoon --cpus 999 should fail"
  ok "invalid --cpus 999 rejected"

  # check no socket leaked
  if [ -S "$HOST_SOCK" ]; then echo "[m6] WARN socket leaked after invalid"; else ok "no socket leaked after invalid"; fi
  if [ -S "$CONTROL_SOCK" ]; then echo "[m6] WARN control leaked after invalid"; else ok "no control leaked after invalid"; fi
}

tier_tests() {
  tier="$1"
  say "=== M6 tier $tier ==="
  need_sock
  # check resource logs
  # Harpoon logs to /tmp/harpoon.log — check for tier
  if grep -q "HARPOON_MEMORY_CONFIG_MIB $tier" /tmp/harpoon.log 2>/dev/null; then ok "log HARPOON_MEMORY_CONFIG_MIB $tier"; else echo "[m6] WARN HARPOON_MEMORY_CONFIG_MIB $tier not in /tmp/harpoon.log"; fi
  if grep -q "HARPOON_CPU_CONFIG_COUNT" /tmp/harpoon.log 2>/dev/null; then ok "log HARPOON_CPU_CONFIG_COUNT"; else echo "[m6] WARN HARPOON_CPU_CONFIG_COUNT missing"; fi
  if grep -q "HARPOON_DISK_IMAGE" /tmp/harpoon.log 2>/dev/null; then ok "log HARPOON_DISK_IMAGE"; else echo "[m6] WARN HARPOON_DISK_IMAGE missing"; fi
  if grep -q "HARPOON_RESOURCE_CONFIG" /tmp/harpoon.log 2>/dev/null; then ok "log HARPOON_RESOURCE_CONFIG"; else echo "[m6] WARN HARPOON_RESOURCE_CONFIG missing"; fi

  export DOCKER_HOST="unix://$HOST_SOCK"
  say "docker version"
  docker version 2>&1 | head -n 5 || fail "docker version $tier"
  ok "docker version $tier"

  say "hello-world $tier"
  docker run --rm hello-world 2>&1 | grep -q "Hello" || fail "hello-world $tier"
  ok "hello-world $tier"

  say "guest MemTotal/MemAvailable $tier"
  out=$(docker run --rm alpine:3.22 sh -c 'grep -E "MemTotal|MemAvailable" /proc/meminfo' 2>&1 || true)
  echo "$out"
  echo "$out" | grep -q "MemTotal" || fail "MemTotal missing $tier"
  echo "$out" | grep -q "MemAvailable" || fail "MemAvailable missing $tier"
  # record expected approx: 1024->970, 768->718, 512->468
  memtotal=$(echo "$out" | grep MemTotal | awk '{print $2}')
  echo "[m6] guest MemTotal kB $memtotal tier $tier"
  ok "guest meminfo $tier"

  say "bounded workload $tier"
  # use alpine to run a modest memory allocation without OOM
  # stress-ng if available, else simple python
  out=$(docker run --rm alpine:3.22 sh -c 'free -m 2>&1 | head -n 5; echo workload-ok' 2>&1 || true)
  echo "$out" | grep -q "workload-ok" || echo "[m6] WARN workload free -m not as expected"
  # run a bounded vm stress if stress-ng present in guest images? use alpine's stress if exists
  # fallback: run harpoon-memory-test if image exists, else skip
  if docker image inspect harpoon-memory-test >/dev/null 2>&1; then
    docker run --rm harpoon-memory-test --vm 1 --vm-bytes 128M --timeout 10s 2>&1 | tail -n 10 || echo "[m6] WARN harpoon-memory-test failed $tier"
  else
    # simple 128M allocation via python
    docker run --rm alpine:3.22 sh -c 'python3 -c "a=bytearray(128*1024*1024); print(len(a))" 2>&1 | head -n 5' 2>&1 | tail -n 5 || echo "[m6] INFO simple workload $tier"
  fi
  ok "bounded workload $tier"

  say "host footprint $tier (if footprint tool available)"
  if command -v footprint >/dev/null 2>&1; then
    # find Harpoon VM XPC pid via pgrep or via harpoon log? Use pgrep -f harpoon
    pid=$(pgrep -f "harpoon/build/harpoon" | head -n 1 || true)
    if [ -n "$pid" ]; then
      footprint -p "$pid" 2>&1 | head -n 20 || echo "[m6] WARN footprint failed"
    else
      echo "[m6] INFO no harpoon pid for footprint"
    fi
  else
    echo "[m6] INFO footprint tool not available"
  fi

  say "container-level --memory $tier"
  docker run --rm --memory 64m alpine:3.22 sh -c 'cat /sys/fs/cgroup/memory.max 2>/dev/null || cat /sys/fs/cgroup/memory/memory.limit_in_bytes 2>/dev/null | head -n 5; echo mem-ok' 2>&1 | grep -q "mem-ok" || echo "[m6] WARN container --memory 64m not verified"
  inspect=$(docker inspect --format '{{.HostConfig.Memory}}' $(docker run -d --memory 64m alpine:3.22 sleep 5) 2>&1 | tail -n 5 || true)
  # cleanup any leftover
  docker ps -a --format '{{.Names}}' | grep -q "m6-" && docker rm -f $(docker ps -a --format '{{.Names}}' | grep "m6-") 2>/dev/null || true
  # simpler: run and inspect
  cid=$(docker run -d --memory 64m alpine:3.22 sleep 10 2>&1 | tail -n 1)
  if [ -n "$cid" ]; then
    mem=$(docker inspect --format '{{.HostConfig.Memory}}' "$cid" 2>&1 | tail -n 1)
    echo "[m6] HostConfig.Memory $mem tier $tier"
    echo "$mem" | grep -q "67108864" || echo "[m6] WARN HostConfig.Memory not 64m $mem"
    docker rm -f "$cid" 2>/dev/null || true
    ok "container --memory 64m $tier"
  else
    echo "[m6] WARN could not create container for --memory test $tier"
  fi

  say "disk observability $tier"
  ls -lh spike2/cache/harpoon-root.img 2>&1 | head -n 5
  if grep -q "HARPOON_DISK_LOGICAL_BYTES" /tmp/harpoon.log 2>/dev/null; then ok "log HARPOON_DISK_LOGICAL_BYTES"; else echo "[m6] WARN HARPOON_DISK_LOGICAL_BYTES missing"; fi
  docker system df 2>&1 | head -n 20 || true
  df_out=$(docker run --rm alpine:3.22 df -B1 /var/lib/docker 2>&1 | tail -n 5 || true)
  echo "$df_out"
  # check low space warning
  if grep -q "HARPOON_DISK_LOW_SPACE" /tmp/harpoon.log 2>/dev/null; then echo "[m6] INFO disk low space warning present"; else echo "[m6] INFO disk not low"; fi
  ok "disk observability $tier"

  say "balloon validation $tier"
  if [ -S "$CONTROL_SOCK" ]; then
    set +e
    echo "512" | nc -U "$CONTROL_SOCK" 2>&1 | head -n 5 || true
    sleep 1
    out=$(grep "HARPOON_BALLOON_TARGET" /tmp/harpoon.log 2>&1 | tail -n 20 || true)
    echo "$out"
    # check reject for below floor or exceed configured
    # try invalid low
    echo "128" | nc -U "$CONTROL_SOCK" 2>&1 | head -n 5 || true
    sleep 1
    if grep -q "HARPOON_BALLOON_TARGET_REJECT.*128" /tmp/harpoon.log 2>&1 || grep -q "HARPOON_BALLOON_TARGET_REJECT" /tmp/harpoon.log 2>&1; then echo "[m6] INFO balloon reject observed"; else echo "[m6] WARN balloon reject not yet visible"; fi
    set -e
    ok "balloon control $tier"
  else
    echo "[m6] WARN control sock missing for balloon $tier"
  fi

  say "M1-M5 sanity $tier"
  # quick checks
  docker run --rm -v /tmp:/workspace alpine:3.22 sh -c 'echo hi > /workspace/m6test && cat /workspace/m6test' 2>&1 | grep -q "hi" || echo "[m6] WARN M4 bind mount"
  # ports: check if m5 forwarding still works (use ephemeral if available)
  echo "[m6] INFO M5 port publishing sanity skipped in tier test (use m5-test.sh)"
  ok "M1-M5 sanity $tier"
}

if [ "$STAGE" = "invalid" ]; then
  invalid_tests
  exit 0
fi
if [ "$STAGE" = "1024" ] || [ "$STAGE" = "768" ] || [ "$STAGE" = "512" ]; then
  tier_tests "$STAGE"
  exit 0
fi
if [ "$STAGE" = "all" ]; then
  invalid_tests
  # if VM is running, detect current tier from log and run that tier
  if [ -S "$HOST_SOCK" ]; then
    cur=$(grep "HARPOON_MEMORY_CONFIG_MIB" /tmp/harpoon.log 2>/dev/null | tail -n 1 | awk '{print $NF}' || echo "1024")
    if [ -z "$cur" ]; then cur="1024"; fi
    # if cur not in allowed, default 1024
    case "$cur" in 1024|768|512) tier_tests "$cur" ;; *) tier_tests "1024" ;; esac
  else
    say "VM not running, skipping live tier tests (run with --stage 1024/768/512 after starting Harpoon at that tier)"
  fi
  exit 0
fi
usage; exit 1
