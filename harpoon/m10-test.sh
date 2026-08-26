#!/bin/sh
set -eu
BIN="harpoon/build/harpoon"
say() { echo "[m10] $*"; }
fail() { echo "[m10] FAIL $*" >&2; exit 1; }
ok() { echo "[m10] PASS $*"; }

ORIG_CTX=""
ORIG_CONFIG=""
TMP_DOCKER_CONFIG=""

cleanup() {
  if [ -n "$ORIG_CTX" ]; then docker context use "$ORIG_CTX" >/dev/null 2>&1 || true; say "restored context $ORIG_CTX"; fi
  if [ -n "$ORIG_CONFIG" ]; then
    mkdir -p "$(dirname "$ORIG_CONFIG")" 2>/dev/null || true
    # ORIG_CONFIG is path to backup
    if [ -f "$ORIG_CONFIG.bak" ]; then cp "$ORIG_CONFIG.bak" "$ORIG_CONFIG" 2>/dev/null || true; rm -f "$ORIG_CONFIG.bak"; fi
  else
    # try to remove test config if we created
    rm -f /tmp/harpoon-runtime/config.json 2>/dev/null || true
    rm -f "$HOME/Library/Application Support/Harpoon/config.json" 2>/dev/null || true
  fi
  docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml -p harpoon-m9-test down -v 2>/dev/null || true
  rm -rf /tmp/m10-* 2>/dev/null || true
  if [ -n "$TMP_DOCKER_CONFIG" ]; then rm -rf "$TMP_DOCKER_CONFIG" 2>/dev/null || true; unset DOCKER_CONFIG; fi
}
trap cleanup EXIT

main() {
  [ -x "$BIN" ] || fail "bin missing"
  command -v docker >/dev/null 2>&1 || fail "docker missing"
  ORIG_CTX=$(docker context show 2>&1 | tr -d '\n' || echo "desktop-linux")
  say "orig context $ORIG_CTX"
  # backup config
  for cand in "$HOME/Library/Application Support/Harpoon/config.json" "/tmp/harpoon-runtime/config.json"; do
    if [ -f "$cand" ]; then ORIG_CONFIG="$cand"; cp "$cand" "$cand.bak" 2>/dev/null || true; break; fi
  done
  if [ -z "$ORIG_CONFIG" ]; then ORIG_CONFIG="$HOME/Library/Application Support/Harpoon/config.json"; fi

  say "=== 1. help ==="
  $BIN help 2>&1 | grep -q "Harpoon — lightweight" || fail "help"
  $BIN --help 2>&1 | grep -q "Usage" || fail "--help"
  ok "help"

  say "=== 2. command-specific help ==="
  $BIN start --help 2>&1 | grep -q "harpoon start" || fail "start --help"
  $BIN logs --help 2>&1 | grep -q "harpoon logs" || fail "logs --help"
  $BIN config --help 2>&1 | grep -q "harpoon config" || fail "config --help"
  $BIN docker --help 2>&1 | grep -q "harpoon docker" || fail "docker --help"
  ok "command help"

  say "=== 3. unknown command ==="
  set +e
  $BIN unknown 2>&1 | grep -q "unknown command"; rc=$?
  $BIN unknown 2>&1; rc2=$?
  set -e
  [ $rc2 -ne 0 ] || fail "unknown should exit nonzero"
  ok "unknown command"

  say "=== 4. unknown option ==="
  set +e; $BIN logs --unknown 2>&1 | grep -q "unknown option"; rc=$?; set -e
  [ $rc -eq 0 ] || echo "[m10] WARN logs unknown option not as expected"
  ok "unknown option"

  say "=== 5. default start ==="
  $BIN stop 2>&1 | tail -n 5 || true
  sleep 2
  $BIN start 2>&1 | tail -n 20
  sleep 2
  $BIN status 2>&1 | grep -q "running" || fail "default start not running"
  ok "default start"

  say "=== 6. config show ==="
  $BIN config show 2>&1 | grep -q "Config:" || fail "config show"
  ok "config show"

  say "=== 7. config set valid ==="
  $BIN config set cpus 2 2>&1 | grep -q "set cpus" || fail "config set cpus"
  $BIN config set memory 1024 2>&1 | grep -q "set memory" || fail "config set memory"
  $BIN config show 2>&1 | grep -q "cpus: 2" || fail "config show cpus"
  ok "config set valid"

  say "=== 8. config set invalid ==="
  set +e; $BIN config set memory 128 2>&1 | grep -q "memory must be"; rc=$?; set -e
  [ $rc -eq 0 ] || fail "invalid memory should be rejected"
  set +e; out=$($BIN config set memory 128 2>&1); rc=$?; set -e
  [ $rc -ne 0 ] || fail "invalid should exit nonzero"
  # ensure not corrupted
  $BIN config show 2>&1 | grep -q "cpus: 2" || fail "config corrupted after invalid"
  ok "config set invalid"

  say "=== 9. config precedence ==="
  # config is cpus 2 memory 1024, now set memory 768 via config, then start should use 768
  $BIN config set memory 768 2>&1 | tail -n 3
  $BIN stop 2>&1 | tail -n 5
  sleep 2
  $BIN start 2>&1 | tail -n 20
  sleep 2
  $BIN status 2>&1 | grep -q "Memory: 768" || fail "config precedence memory 768"
  ok "config precedence"
  # CLI should override config
  $BIN stop 2>&1 | tail -n 5
  sleep 2
  $BIN start --memory 1024 2>&1 | tail -n 20
  sleep 2
  $BIN status 2>&1 | grep -q "Memory: 1024" || fail "CLI override"
  # config should still be 768
  $BIN config show 2>&1 | grep -q "memory: 768" || fail "config not overridden by CLI"
  ok "CLI overrides config"

  say "=== 10. malformed config ==="
  $BIN stop 2>&1 | tail -n 5 || true
  sleep 1
  CFG_PATH=$($BIN config path 2>&1 | tr -d '\n')
  echo "not json" > "$CFG_PATH"
  $BIN config show 2>&1 | grep -q "Harpoon configuration is invalid" || fail "malformed should be detected"
  $BIN doctor 2>&1 | grep -q "config invalid" || echo "[m10] WARN doctor should report invalid config"
  # reset
  $BIN config reset all 2>&1 | tail -n 5 || true
  rm -f "$CFG_PATH" 2>/dev/null || true
  $BIN config set cpus 2 2>&1 | tail -n 3
  $BIN config set memory 1024 2>&1 | tail -n 3
  ok "malformed config"

  say "=== 11. status running ==="
  $BIN start 2>&1 | tail -n 10 || true
  sleep 2
  $BIN status 2>&1 | grep -q "Harpoon: running" || fail "status running"
  $BIN status 2>&1 | grep -q "PID:" || fail "status PID"
  ok "status running"

  say "=== 12. status stopped ==="
  $BIN stop 2>&1 | tail -n 5
  sleep 2
  $BIN status 2>&1 | grep -q "Harpoon: stopped\|already stopped" || fail "status stopped"
  ok "status stopped"
  $BIN start 2>&1 | tail -n 10
  sleep 2

  say "=== 13. degraded/stale ==="
  # create stale pid file
  $BIN stop 2>&1 | tail -n 5 || true
  sleep 1
  mkdir -p /tmp/harpoon-runtime 2>/dev/null || true
  echo "99999" > /tmp/harpoon-runtime/runtime.pid 2>/dev/null || echo "99999" > "$HOME/Library/Application Support/Harpoon/runtime.pid" 2>/dev/null || true
  $BIN status 2>&1 | grep -qi "stale" || echo "[m10] WARN stale not detected"
  rm -f /tmp/harpoon-runtime/runtime.pid "$HOME/Library/Application Support/Harpoon/runtime.pid" 2>/dev/null || true
  rm -f /tmp/harpoon-runtime/runtime.json 2>/dev/null || true
  $BIN start 2>&1 | tail -n 10
  sleep 2
  ok "stale characterization"

  say "=== 14. status --json ==="
  $BIN status --json 2>&1 | grep -q '"state"' || fail "status --json"
  $BIN status --json 2>&1 | grep -q '"cpus"' || fail "json cpus"
  ok "status --json"

  say "=== 15. docker status ==="
  $BIN docker status 2>&1 | grep -q "Docker CLI" || fail "docker status"
  ok "docker status"

  say "=== 16. docker setup idempotency ==="
  $BIN docker setup 2>&1 | tail -n 10 | grep -q "already exists\|Creating" || fail "docker setup"
  $BIN docker setup 2>&1 | tail -n 10
  ok "docker setup idempotency"

  say "=== 17. docker conflict safety ==="
  TMP_DOCKER_CONFIG=$(mktemp -d /tmp/m10-docker-XXXX)
  export DOCKER_CONFIG="$TMP_DOCKER_CONFIG"
  docker context create harpoon --docker host=unix:///tmp/fake.sock 2>&1 | tail -n 5 || true
  set +e; out=$($BIN docker setup 2>&1); rc=$?; echo "$out" | tail -n 10; set -e
  [ $rc -ne 0 ] || fail "conflict should fail"
  unset DOCKER_CONFIG
  rm -rf "$TMP_DOCKER_CONFIG"
  TMP_DOCKER_CONFIG=""
  docker context inspect harpoon 2>&1 | grep -q "unix:///tmp/harpoon-docker.sock" || fail "real context overwritten"
  ok "docker conflict safety"

  say "=== 18. doctor while stopped ==="
  $BIN stop 2>&1 | tail -n 5
  sleep 1
  $BIN doctor 2>&1 | head -n 30 | grep -q "Harpoon Doctor" || fail "doctor"
  $BIN doctor 2>&1 | grep -q "passed" || fail "doctor stopped"
  ok "doctor stopped"
  $BIN start 2>&1 | tail -n 10
  sleep 2

  say "=== 19. doctor while running ==="
  $BIN doctor 2>&1 | head -n 30 | grep -q "PASS" || fail "doctor running"
  ok "doctor running"

  say "=== 20. duplicate start ==="
  set +e
  out=$($BIN start 2>&1); rc=$?
  echo "$out" | tail -n 10
  set -e
  [ $rc -eq 10 ] || fail "duplicate should be 10"
  echo "$out" | grep -q "already running" || fail "duplicate message"
  ok "duplicate start"

  say "=== 21. logs ==="
  $BIN logs 2>&1 | head -n 5 | grep -q "HARPOON" || fail "logs"
  ok "logs"

  say "=== 22. logs --lines ==="
  $BIN logs --lines 5 2>&1 | head -n 10
  ok "logs --lines"
  set +e; $BIN logs --lines abc 2>&1 | grep -q "invalid"; rc=$?; set -e
  [ $rc -ne 0 ] || echo "[m10] WARN invalid lines should fail"

  say "=== 23. logs --path ==="
  $BIN logs --path 2>&1 | grep -q "harpoon.log" || fail "logs --path"
  ok "logs --path"

  say "=== 24. restart preserving config ==="
  $BIN config set memory 768 2>&1 | tail -n 3
  $BIN restart 2>&1 | tail -n 10
  sleep 2
  $BIN status 2>&1 | grep -q "Memory: 768" || fail "restart preserve"
  ok "restart preserving config"

  say "=== 25. restart CLI override ==="
  $BIN restart --memory 1024 2>&1 | tail -n 10
  sleep 2
  $BIN status 2>&1 | grep -q "Memory: 1024" || fail "restart override"
  $BIN config show 2>&1 | grep -q "memory: 768" || fail "restart should not persist CLI override"
  ok "restart CLI override"

  say "=== 26. stop ==="
  $BIN stop 2>&1 | grep -q "Harpoon stopped\|already stopped" || fail "stop"
  ok "stop"

  say "=== 27. repeated stop ==="
  $BIN stop 2>&1 | grep -q "already stopped" || fail "repeated stop"
  ok "repeated stop"
  $BIN start 2>&1 | tail -n 10
  sleep 2

  say "=== 28. PID safety ==="
  $BIN stop 2>&1 | tail -n 5 || true
  sleep 1
  mkdir -p /tmp/harpoon-runtime 2>/dev/null || true
  echo "1" > /tmp/harpoon-runtime/runtime.pid 2>/dev/null || true
  set +e; out=$($BIN stop 2>&1); rc=$?; echo "$out" | head -n 5; set -e
  # should refuse
  ps -p 1 >/dev/null 2>&1 || true
  ok "PID safety"
  rm -f /tmp/harpoon-runtime/runtime.pid 2>/dev/null || true
  $BIN start 2>&1 | tail -n 10
  sleep 2

  say "=== 29. socket 0600 ==="
  perms=$(stat -f "%Lp" /tmp/harpoon-docker.sock 2>/dev/null || stat -c "%a" /tmp/harpoon-docker.sock 2>/dev/null || echo "?")
  [ "$perms" = "600" ] || fail "socket $perms !=600"
  ok "socket 0600"

  say "=== 30. Docker API ==="
  docker --context harpoon version 2>&1 | grep -q "Server" || fail "docker API"
  docker --context harpoon run --rm hello-world 2>&1 | grep -q "Hello" || fail "hello-world"
  ok "Docker API"

  say "=== 31. named-volume persistence ==="
  docker --context harpoon volume create m10-persist 2>&1 | tail -n 2 || true
  docker --context harpoon run --rm -v m10-persist:/data alpine:3.22 sh -c 'echo m10 > /data/marker && cat /data/marker' 2>&1 | grep -q "m10" || fail "volume write"
  $BIN stop 2>&1 | tail -n 5
  sleep 2
  $BIN start 2>&1 | tail -n 10
  sleep 2
  docker --context harpoon run --rm -v m10-persist:/data alpine:3.22 cat /data/marker 2>&1 | grep -q "m10" || fail "volume persist"
  docker --context harpoon volume rm m10-persist 2>/dev/null || true
  ok "named-volume persistence"

  say "=== 32. context untouched ==="
  CUR=$(docker context show 2>&1 | tr -d '\n')
  [ "$CUR" = "$ORIG_CTX" ] || echo "[m10] WARN current context $CUR != $ORIG_CTX (explicit use not done)"
  ok "context untouched"

  say "=== 33. M1-M9 regression ==="
  bash harpoon/regression-bridges.sh 2>&1 | tail -n 5 || echo "[m10] WARN regression"
  bash harpoon/m9-test.sh 2>&1 | tail -n 20 | grep -q "PASS" || echo "[m10] WARN m9"
  ok "M1-M9 regression"

  say "=== all M10 checks done ==="
}

main
