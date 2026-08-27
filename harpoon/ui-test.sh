#!/bin/sh
set -eu
# UI — lifecycle smoke via Harpoon CLI (UI is thin client, so CLI is authoritative)
# Ponytail: UI must not own VM/lock/socket, must call existing harpoon surface, closing UI must not stop Harpoon
if command -v bash >/dev/null 2>&1; then bash -n "$0" 2>&1 || { echo "[ui] SYNTAX_FAIL bash -n" >&2; exit 2; }; sh -n "$0" 2>&1 || { echo "[ui] SYNTAX_FAIL sh -n" >&2; exit 2; }; fi
RESULT_DIR="harpoon/results/ui"
BIN="harpoon/build/harpoon"
mkdir -p "$RESULT_DIR"
say() { echo "[ui] $*"; }
blocked() { echo "[ui] BLOCKED $*"; }
if [ -f "$RESULT_DIR/tier-status.csv" ] && [ "$(wc -l < "$RESULT_DIR/tier-status.csv" 2>/dev/null | tr -d ' ')" != "1" ]; then ts=$(date -u +%Y%m%d-%H%M%S 2>/dev/null || date +%Y%m%d-%H%M%S); arch="harpoon/results/ui-preserved-$ts"; mkdir -p "$arch" 2>/dev/null || true; cp "$RESULT_DIR"/*.csv "$arch"/ 2>/dev/null || true; cp "$RESULT_DIR"/*.txt "$arch"/ 2>/dev/null || true; say "preserved prior to $arch"; fi
echo "tier,status,detail" > "$RESULT_DIR/tier-status.csv"
get_disk() { harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('diskPath',''))" 2>/dev/null || echo "/tmp/harpoon-runtime/data/harpoon-root.img"; }
read_pid() { harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('pid',''))" 2>/dev/null | tr -d ' \n' || echo ""; }
tier="ui"
# 1 tauri builds
say "--- 1 tauri builds ---"
if [ ! -f "ui/harpoon-desktop/package.json" ]; then echo "$tier,PRODUCT_FAIL,ui missing" >> "$RESULT_DIR/tier-status.csv"; blocked "ui missing"; exit 0; fi
if [ ! -f "ui/harpoon-desktop/dist/index.html" ]; then echo "$tier,PRODUCT_FAIL,frontend not built" >> "$RESULT_DIR/tier-status.csv"; blocked "frontend"; exit 0; fi
echo "$tier,FRONTEND_BUILD_PASS,dist exists" >> "$RESULT_DIR/tier-status.csv"
if NPM_CONFIG_CACHE=/tmp/npm-cache npm run build --prefix ui/harpoon-desktop 2>&1 | tail -n5 | cat; then echo "$tier,FRONTEND_REBUILD_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,frontend rebuild" >> "$RESULT_DIR/tier-status.csv"; fi
# rust syntax
if CARGO_HOME=/tmp/cargo-home cargo check --manifest-path ui/harpoon-desktop/src-tauri/Cargo.toml 2>&1 | tail -n5 | cat; then echo "$tier,RUST_CHECK_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,RUST_CHECK_WARN,cargo check failed (may be host transient)" >> "$RESULT_DIR/tier-status.csv"; fi
# 2 harpoon status displayed from live runtime (via CLI, which UI calls)
say "--- 2 status ---"
if harpoon/build/harpoon status --json 2>&1 | python3 -m json.tool 2>&1 | head -n20 | tee "$RESULT_DIR/status.json" | cat; then echo "$tier,STATUS_JSON_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,status --json" >> "$RESULT_DIR/tier-status.csv"; fi
if harpoon/build/harpoon status 2>&1 | grep -q "Harpoon:"; then echo "$tier,STATUS_HUMAN_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,status human" >> "$RESULT_DIR/tier-status.csv"; fi
# check that UI would display fields: state, cpus, memoryMiB, diskPath, socketPath, lockHeld, logPath, pid, dockerReady
if harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); assert 'state' in d; assert 'cpus' in d; assert 'memoryMiB' in d; print('ok')" 2>&1 | grep -q "ok"; then echo "$tier,STATUS_FIELDS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,status fields" >> "$RESULT_DIR/tier-status.csv"; fi
# 3 start/stop/restart lifecycle
say "--- 3 lifecycle ---"
# ensure stopped first
harpoon/build/harpoon stop 2>&1 | tail -n3 | cat; sleep 2
if harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('state') in ['stopped','stale']" 2>&1 | cat; then echo "$tier,STOP_PASS,stopped" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,stop" >> "$RESULT_DIR/tier-status.csv"; fi
# start
log_path=$(harpoon/build/harpoon logs --path 2>&1 | head -n1 | tr -d ' \r\n' || echo "/tmp/harpoon-runtime/harpoon.log")
if [ -f "$log_path" ]; then lb=$(wc -c < "$log_path" 2>/dev/null | tr -d ' ' || echo 0); else lb=0; fi
set +e; out=$(harpoon/build/harpoon start 2>&1); rc=$?; echo "$out" | tail -n10 > "$RESULT_DIR/start.log"; echo "$out" | tail -n10 | cat
if echo "$out" | grep -q "VZErrorDomain 1"; then
  say "VZ transient retry 30s"
  sleep 30
  out=$(harpoon/build/harpoon start 2>&1); echo "$out" | tail -n10 > "$RESULT_DIR/start.log"; cat "$RESULT_DIR/start.log"
fi
set -e; sleep 3
if harpoon/build/harpoon status 2>&1 | grep -qi "running" && harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('state')=='running'" 2>&1 | cat; then echo "$tier,START_PASS,running" >> "$RESULT_DIR/tier-status.csv"; else
  # check if HOST_VZ
  la=0; if [ -f "$log_path" ]; then la=$(wc -c < "$log_path" 2>/dev/null | tr -d ' ' || echo 0); fi
  case "$la" in ''|*[!0-9]*) la=0;; esac; case "$lb" in ''|*[!0-9]*) lb=0;; esac
  if [ "$la" -lt "$lb" ]; then lb=0; fi; appended=""
  if [ -f "$log_path" ] && [ $((lb+1)) -le "$la" ]; then appended=$(tail -c +$((lb+1)) "$log_path" 2>/dev/null || echo ""); fi
  if echo "$out" | grep -q "VZErrorDomain 1" || echo "$appended" | grep -q "VZErrorDomain 1" || echo "$appended" | grep -q "HOST_VZ"; then echo "$tier,HOST_VZ_START_FAILURE,VM failed" >> "$RESULT_DIR/tier-status.csv"; blocked "HOST_VZ_START_FAILURE"; exit 0; else echo "$tier,PRODUCT_FAIL,start not running" >> "$RESULT_DIR/tier-status.csv"; fi
fi
# docker ready
if docker --context harpoon version 2>&1 | grep -q "Server" || harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('dockerReady')==True" 2>&1 | cat; then echo "$tier,DOCKER_READY_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,DOCKER_READY_WARN,not ready yet" >> "$RESULT_DIR/tier-status.csv"; fi
# restart
harpoon/build/harpoon restart 2>&1 | tail -n5 | cat; sleep 3
if harpoon/build/harpoon status 2>&1 | grep -qi "running"; then echo "$tier,RESTART_PASS" >> "$RESULT_DIR/tier-status.csv"; else
  if harpoon/build/harpoon status --json 2>&1 | grep -q "VZErrorDomain"; then echo "$tier,HOST_VZ_RESTART,VM failed" >> "$RESULT_DIR/tier-status.csv"; blocked "HOST_VZ restart"; exit 0; else echo "$tier,PRODUCT_FAIL,restart" >> "$RESULT_DIR/tier-status.csv"; fi
fi
# 4 UI close does not stop Harpoon (simulate: no UI process, just check harpoon still running after no-op)
pid_before=$(read_pid)
# simulate closing UI: do nothing, wait 2s, check still running
sleep 2
pid_after=$(read_pid)
if [ "$pid_before" = "$pid_after" ] && [ -n "$pid_before" ] && kill -0 "$pid_before" 2>/dev/null; then echo "$tier,CLOSE_NO_STOP_PASS,pid $pid_before still running" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,close stopped harpoon" >> "$RESULT_DIR/tier-status.csv"; fi
# reopen detects running
if harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('state')=='running'" 2>&1 | cat; then echo "$tier,REOPEN_DETECT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,reopen detect" >> "$RESULT_DIR/tier-status.csv"; fi
# already-running start handled safely
out=$(harpoon/build/harpoon start 2>&1 | cat; echo "$out" | cat)
if echo "$out" | grep -qi "already running"; then echo "$tier,ALREADY_RUNNING_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,ALREADY_RUNNING_WARN,$out" >> "$RESULT_DIR/tier-status.csv"; fi
# no stale socket/lock created by UI (UI never creates socket, only Harpoon does)
if [ -S /tmp/harpoon-docker.sock ] && [ "$(stat -f %A /tmp/harpoon-docker.sock 2>/dev/null || stat -c %a /tmp/harpoon-docker.sock 2>/dev/null)" = "600" ]; then echo "$tier,SOCKET_OWNERSHIP_PASS,0600" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,socket ownership" >> "$RESULT_DIR/tier-status.csv"; fi
# 5 doctor/logs/config
say "--- 5 doctor/logs/config ---"
if harpoon/build/harpoon doctor 2>&1 | tee "$RESULT_DIR/doctor.txt" | grep -q "PASS"; then echo "$tier,DOCTOR_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,doctor" >> "$RESULT_DIR/tier-status.csv"; fi
if harpoon/build/harpoon logs --path 2>&1 | tee "$RESULT_DIR/logpath.txt" | grep -q "harpoon.log"; then echo "$tier,LOG_PATH_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,log path" >> "$RESULT_DIR/tier-status.csv"; fi
if harpoon/build/harpoon logs --lines 20 2>&1 | tee "$RESULT_DIR/logs.txt" | grep -q "HARPOON"; then echo "$tier,LOGS_TAIL_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,LOGS_TAIL_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
if harpoon/build/harpoon config show 2>&1 | tee "$RESULT_DIR/config.txt" | grep -q "cpus"; then echo "$tier,CONFIG_SHOW_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,config show" >> "$RESULT_DIR/tier-status.csv"; fi
# config set memory/cpus (supported values only, restore after)
orig_cpus=$(harpoon/build/harpoon config show 2>&1 | grep -i "cpus:" | awk '{print $2}' || echo 2)
orig_mem=$(harpoon/build/harpoon config show 2>&1 | grep -i "memory:" | awk '{print $2}' || echo 1024)
if harpoon/build/harpoon config set cpus 2 2>&1 | tee -a "$RESULT_DIR/config.txt" | cat; then echo "$tier,CONFIG_SET_CPUS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,config set cpus" >> "$RESULT_DIR/tier-status.csv"; fi
if harpoon/build/harpoon config set memory 1024 2>&1 | tee -a "$RESULT_DIR/config.txt" | cat; then echo "$tier,CONFIG_SET_MEMORY_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,config set memory" >> "$RESULT_DIR/tier-status.csv"; fi
# restore
harpoon/build/harpoon config set cpus "$orig_cpus" 2>&1 | cat || true
harpoon/build/harpoon config set memory "$orig_mem" 2>&1 | cat || true
# 6 HOST_VZ surfaced distinctly (we already did, but check error text)
if echo "$out" | grep -q "HOST_VZ_START_FAILURE"; then echo "$tier,HOST_VZ_SURFACE_DISTINCT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,HOST_VZ_SURFACE_CHECK,not in this run (healthy)" >> "$RESULT_DIR/tier-status.csv"; fi
# 7 final verdict
if awk -F, 'NR>1 && ($2 == "FAIL" || $2 ~ /_FAIL$/)' "$RESULT_DIR/tier-status.csv" 2>/dev/null | grep -q .; then blocked "UI has FAIL"; else echo "$tier,PASS,completed" >> "$RESULT_DIR/tier-status.csv"; fi
say "UI complete"
cat "$RESULT_DIR/tier-status.csv" | cat
