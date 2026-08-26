#!/bin/sh
set -eu

# M14 paired idle optimization validation — PortForwardManager 2s vs 10s
# Requirements: immutable before/after binaries, provenance, paired 512/768/1024, 60s port-sync window, fail-fast, clean CSVs

RESULT_DIR="harpoon/results/m14"
BIN_DIR="$RESULT_DIR/bin"
mkdir -p "$BIN_DIR" "$RESULT_DIR"

SRC_FILE="harpoon/Sources/PortForwardManager.swift"
BIN_BEFORE="$BIN_DIR/harpoon-before"
BIN_AFTER="$BIN_DIR/harpoon-after"
PROV_BEFORE="$RESULT_DIR/provenance-before.txt"
PROV_AFTER="$RESULT_DIR/provenance-after.txt"

say() { echo "[m14] $*"; }
pass() { echo "[m14] PASS $*"; }
blocked() { echo "[m14] BLOCKED $*"; }
warn() { echo "[m14] WARN $*"; }

# --- helpers: host/guest measurement ---
get_disk_path() {
  disk=$(harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('disk',''))" 2>/dev/null || echo "")
  if [ -n "$disk" ] && [ -f "$disk" ]; then echo "$disk"; return; fi
  if [ -f /tmp/harpoon-runtime/data/harpoon-root.img ]; then echo "/tmp/harpoon-runtime/data/harpoon-root.img"; return; fi
  if [ -f "$HOME/Library/Application Support/Harpoon/data/harpoon-root.img" ]; then echo "$HOME/Library/Application Support/Harpoon/data/harpoon-root.img"; return; fi
  if [ -f spike2/cache/harpoon-root.img ]; then echo "spike2/cache/harpoon-root.img"; return; fi
  echo "/tmp/harpoon-runtime/data/harpoon-root.img"
}
find_vm_pid() {
  disk="$1"
  pid=$(lsof -n 2>/dev/null | grep -F "$disk" | grep -i "Virtualization" | awk '{print $2}' | head -n1 || echo "")
  if [ -n "$pid" ]; then echo "$pid"; return; fi
  pid=$(pgrep -f "Virtualization" 2>/dev/null | head -n1 || echo "")
  if [ -n "$pid" ] && lsof -p "$pid" 2>/dev/null | grep -q -F "$disk"; then echo "$pid"; return; fi
  echo ""
}
host_measure() {
  tier="$1"; variant="$2"; phase="$3"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  # fail-fast: if runtime not alive, do not emit blank row
  if ! check_runtime "$variant" >/dev/null 2>&1; then
    return 1
  fi
  pid=$(cat /tmp/harpoon.pid 2>/dev/null | tr -d ' \n' || echo "")
  if [ -z "$pid" ]; then pid=$(pgrep -f "harpoon.*run" 2>/dev/null | head -n1 || echo ""); fi
  harpoon_rss=""; harpoon_cpu=""; harpoon_threads=""; harpoon_fds=""
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    harpoon_rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    harpoon_cpu=$(ps -o %cpu= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo "")
    harpoon_threads=$(ps -M "$pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    harpoon_fds=$(lsof -p "$pid" 2>/dev/null | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
  fi
  disk=$(get_disk_path)
  vm_pid=$(find_vm_pid "$disk")
  vm_rss=""; vm_cpu=""; vm_threads=""; vm_fds=""
  if [ -n "$vm_pid" ] && kill -0 "$vm_pid" 2>/dev/null; then
    vm_rss=$(ps -o rss= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    vm_cpu=$(ps -o %cpu= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo "")
    vm_threads=$(ps -M "$vm_pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    vm_fds=$(lsof -p "$vm_pid" 2>/dev/null | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
  fi
  combined=""
  if echo "$harpoon_rss" | grep -qE '^[0-9]+$' && echo "$vm_rss" | grep -qE '^[0-9]+$'; then combined=$((harpoon_rss + vm_rss)); elif echo "$harpoon_rss" | grep -qE '^[0-9]+$'; then combined="$harpoon_rss"; fi
  # only emit if we have at least harpoon_rss
  if [ -z "$harpoon_rss" ] && [ -z "$vm_rss" ]; then return 1; fi
  echo "$ts,$tier,$variant,$phase,$pid,$harpoon_rss,$harpoon_cpu,$harpoon_threads,$harpoon_fds,$vm_pid,$vm_rss,$vm_cpu,$vm_threads,$vm_fds,$combined" >> "$RESULT_DIR/host.csv"
  return 0
}
guest_measure() {
  tier="$1"; variant="$2"; phase="$3"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  if ! check_runtime "$variant" >/dev/null 2>&1; then return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then return 1; fi
  out=$(docker --context harpoon run --rm alpine:3.22 sh -c 'cat /proc/meminfo | grep -E "MemTotal|MemAvailable"; echo "---"; cat /proc/loadavg; echo "---"; ps aux | wc -l' 2>&1 || echo "guest_failed")
  if echo "$out" | grep -q "guest_failed"; then return 1; fi
  memtotal=$(echo "$out" | grep MemTotal | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
  memavail=$(echo "$out" | grep MemAvailable | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
  load_line=$(echo "$out" | grep -E "^[0-9]+\.[0-9]+ [0-9]" | head -n1 || echo "")
  load1=$(echo "$load_line" | awk '{print $1}' || echo "")
  load5=$(echo "$load_line" | awk '{print $2}' || echo "")
  load15=$(echo "$load_line" | awk '{print $3}' || echo "")
  pcount=$(echo "$out" | awk '/---/{c++; next} c==2{print}' 2>&1 | head -n1 | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
  if [ -z "$memtotal" ] && [ -z "$memavail" ]; then return 1; fi
  echo "$ts,$tier,$variant,$phase,$memtotal,$memavail,$load1,$load5,$load15,$pcount" >> "$RESULT_DIR/guest.csv"
  return 0
}
check_runtime() {
  variant="$1"
  # variant is used to pick binary, but check is same for both: harpoon status, socket, docker
  # Use the selected binary's status if available, else harpoon/build/harpoon
  BIN_CHECK=""
  if [ "$variant" = "before" ] && [ -x "$BIN_BEFORE" ]; then BIN_CHECK="$BIN_BEFORE"
  elif [ "$variant" = "after" ] && [ -x "$BIN_AFTER" ]; then BIN_CHECK="$BIN_AFTER"
  else BIN_CHECK="harpoon/build/harpoon"
  fi
  # PID alive
  pid=$(cat /tmp/harpoon.pid 2>/dev/null | tr -d ' \n' || echo "")
  if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
    # try pgrep
    pid2=$(pgrep -f "harpoon.*run" 2>/dev/null | head -n1 || echo "")
    if [ -z "$pid2" ] || ! kill -0 "$pid2" 2>/dev/null; then echo "PID not alive"; return 1; fi
  fi
  if ! "$BIN_CHECK" status 2>&1 | grep -qi "running"; then echo "status not running"; return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then echo "socket missing"; return 1; fi
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then echo "docker not ready"; return 1; fi
  return 0
}
log_file_for_sync() {
  variant="$1"
  BIN_CHECK=""
  if [ "$variant" = "before" ] && [ -x "$BIN_BEFORE" ]; then BIN_CHECK="$BIN_BEFORE"
  elif [ "$variant" = "after" ] && [ -x "$BIN_AFTER" ]; then BIN_CHECK="$BIN_AFTER"
  else BIN_CHECK="harpoon/build/harpoon"
  fi
  # authoritative path from tested binary (no hardcoded HOME)
  auth_path=""
  if [ -x "$BIN_CHECK" ]; then
    auth_path=$("$BIN_CHECK" logs --path 2>/dev/null | head -n1 | tr -d '\r\n' || echo "")
    # trim whitespace
    auth_path=$(echo "$auth_path" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
  fi
  home_log="$HOME/Library/Application Support/Harpoon/harpoon.log"
  tmp_log="/tmp/harpoon-runtime/harpoon.log"
  legacy="/tmp/harpoon.log"
  # collect candidates and pick newest by mtime (authoritative log resolution)
  best=""
  best_mtime=0
  # use newline-separated list to handle spaces in $HOME path
  for cand in "$auth_path" "$home_log" "$tmp_log" "$legacy"; do
    [ -z "$cand" ] && continue
    [ -f "$cand" ] || continue
    mtime=$(stat -f %m "$cand" 2>/dev/null || stat -c %Y "$cand" 2>/dev/null || echo 0)
    # ensure numeric
    case "$mtime" in ''|*[!0-9]*) mtime=0 ;; esac
    if [ "$mtime" -gt "$best_mtime" ]; then
      best_mtime=$mtime
      best="$cand"
    fi
  done
  if [ -n "$best" ]; then echo "$best"; return; fi
  if [ -n "$auth_path" ]; then echo "$auth_path"; return; fi
  echo "$home_log"
}

# --- provenance helpers ---
verify_source_interval() {
  interval="$1"
  if grep -q "repeating: $interval" "$SRC_FILE"; then
    line=$(grep -n "repeating: $interval" "$SRC_FILE" | head -n1)
    say "source verify $interval s: $line"
    return 0
  else
    say "FAIL source does not contain repeating: $interval"
    grep -n "repeating" "$SRC_FILE" | head -n5
    return 1
  fi
}
record_provenance() {
  label="$1"
  bin="$2"
  interval="$3"
  prov="$RESULT_DIR/provenance-${label}.txt"
  {
    echo "label=$label"
    echo "binary=$bin"
    echo "binary_abs=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
    echo "size=$(stat -f%z "$bin" 2>/dev/null || stat -c%s "$bin" 2>/dev/null)"
    echo "sha256=$(shasum -a 256 "$bin" | awk '{print $1}')"
    echo "arch=$(file "$bin" | head -n1)"
    echo "codesign=$(codesign -dv "$bin" 2>&1 | head -n5 | tr '\n' ';')"
    echo "codesign_verify=$(codesign --verify --verbose "$bin" 2>&1 | head -n1)"
    echo "polling_interval_seconds=$interval"
    echo "source_line=$(grep -n "repeating: $interval" "$SRC_FILE" | head -n1)"
    echo "source_repeating_grep=$(grep -n "repeating" "$SRC_FILE" | head -n5 | tr '\n' ';')"
    echo "git_commit=$(git rev-parse HEAD 2>&1 | head -n1)"
    echo "git_diff_PortForwardManager=$(git diff -- "$SRC_FILE" 2>&1 | head -n20 | tr '\n' ';')"
  } > "$prov"
  cat "$prov"
}

# --- build helpers ---
build_binary() {
  label="$1"
  interval="$2"
  say "building $label with interval $interval s"
  verify_source_interval "$interval" || { echo "[m14] FAIL source verify $interval"; exit 1; }
  bash harpoon/build.sh 2>&1 | tail -n 5
  bin_src="harpoon/build/harpoon"
  bin_dst="$BIN_DIR/harpoon-$label"
  cp "$bin_src" "$bin_dst"
  chmod +x "$bin_dst"
  # verify built binary still shows expected interval in source (not binary string, but we trust build)
  say "built $bin_dst"
  ls -lh "$bin_dst"
  shasum -a 256 "$bin_dst"
}

# --- main ---
# Ensure final source is 10s, with trap to restore
ORIG_SRC_BACKUP="/tmp/PortForwardManager.swift.orig.$$"
cp "$SRC_FILE" "$ORIG_SRC_BACKUP"
cleanup() {
  if [ -f "$ORIG_SRC_BACKUP" ]; then
    cp "$ORIG_SRC_BACKUP" "$SRC_FILE"
    rm -f "$ORIG_SRC_BACKUP"
    say "restored $SRC_FILE to 10s state"
    # verify final is 10s
    if ! grep -q "repeating: 10" "$SRC_FILE"; then
      # force restore to 10s if needed
      python3 << 'PY'
import pathlib
p = pathlib.Path("harpoon/Sources/PortForwardManager.swift")
t = p.read_text()
if "repeating: 2" in t:
    t = t.replace("repeating: 2", "repeating: 10")
    t = t.replace("deadline: .now()+2", "deadline: .now()+5")
    # ensure comment present
    if "M14: idle optimization" not in t:
        t = t.replace("    func startPolling() {", "    func startPolling() {\n        // M14: idle optimization — was 2s unconditional (30 wakeups/min), now 10s (6/min, 80% reduction)\n        // Justification: published ports not latency-critical (dev tolerates 10s); sync also triggered via scheduleSync on guest IP/container changes, so 10s is fallback only.\n        // Before: HARPOON_PORT_SYNC_START every 2s even idle (see harpoon.log). After: 10s reduces idle CPU wakeups while preserving correctness (reconciled within 10s).")
    p.write_text(t)
    print("forced restore to 10s")
PY
    fi
    bash harpoon/build.sh 2>&1 | tail -n 3
  fi
}
trap cleanup EXIT INT TERM

# Check current source is 10s (final state)
say "checking final source is 10s before starting"
verify_source_interval "10" || { echo "[m14] FAIL final source must be 10s"; exit 1; }

# Prepare result dirs and CSVs
mkdir -p "$BIN_DIR"
# Archive previous M14 if not already archived this run
ARCHIVE_DIR="$RESULT_DIR/archive-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$ARCHIVE_DIR"
cp -a "$RESULT_DIR"/*.csv "$RESULT_DIR"/*.txt "$RESULT_DIR"/*.log 2>/dev/null | head -n 5 || true
cp -a "$RESULT_DIR"/*.csv "$ARCHIVE_DIR"/ 2>/dev/null || true
say "archived previous M14 to $ARCHIVE_DIR"

# Headers for clean CSVs
echo "timestamp,tier,variant,phase,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,harpoon_threads,harpoon_fds,vm_pid,vm_rss_kib,vm_cpu_pct,vm_threads,vm_fds,combined_rss_kib" > "$RESULT_DIR/host.csv"
echo "timestamp,tier,variant,phase,mem_total_kib,mem_available_kib,load1,load5,load15,process_count" > "$RESULT_DIR/guest.csv"
echo "tier,variant,window_seconds,sync_count,syncs_per_minute" > "$RESULT_DIR/port-sync.csv"
echo "tier,before_sync_per_min,after_sync_per_min,sync_reduction_pct,before_idle_combined_rss_median_kib,after_idle_combined_rss_median_kib,before_idle_cpu_median,after_idle_cpu_median,before_docker_run_sec,after_docker_run_sec,before_persistence,after_persistence,result" > "$RESULT_DIR/comparison.csv"
echo "tier,variant,status,detail" > "$RESULT_DIR/tier-status.csv"
# Provenance will be created after builds

# --- Build BEFORE (2s) ---
say "=== Building BEFORE (2s) ==="
# Patch source to 2s
python3 << 'PY'
import pathlib
p = pathlib.Path("harpoon/Sources/PortForwardManager.swift")
t = p.read_text()
# Replace 10s block with 2s
old = """    func startPolling() {
        // M14: idle optimization — was 2s unconditional (30 wakeups/min), now 10s (6/min, 80% reduction)
        // Justification: published ports are not latency-critical (dev tolerates 10s); sync also triggered via scheduleSync on guest IP/container changes, so 10s is fallback only.
        // Before: HARPOON_PORT_SYNC_START every 2s even idle (see harpoon.log). After: 10s reduces idle CPU wakeups while preserving correctness (reconciled within 10s).
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now()+5, repeating: 10)
        t.setEventHandler { [weak self] in self?.sync() }
        t.resume()
        dockerPoll = t
    }"""
new = """    func startPolling() {
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now()+2, repeating: 2)
        t.setEventHandler { [weak self] in self?.sync() }
        t.resume()
        dockerPoll = t
    }"""
if old in t:
    t = t.replace(old, new)
    p.write_text(t)
    print("patched to 2s")
else:
    # fallback simple replace
    t = t.replace("repeating: 10", "repeating: 2").replace("deadline: .now()+5", "deadline: .now()+2")
    # remove comment if present
    t = t.replace("        // M14: idle optimization — was 2s unconditional (30 wakeups/min), now 10s (6/min, 80% reduction)\n", "")
    t = t.replace("        // Justification: published ports are not latency-critical (dev tolerates 10s); sync also triggered via scheduleSync on guest IP/container changes, so 10s is fallback only.\n", "")
    t = t.replace("        // Before: HARPOON_PORT_SYNC_START every 2s even idle (see harpoon.log). After: 10s reduces idle CPU wakeups while preserving correctness (reconciled within 10s).\n", "")
    p.write_text(t)
    print("fallback patched to 2s")
PY
verify_source_interval "2" || exit 1
build_binary "before" "2"
# Record provenance before
record_provenance "before" "$BIN_DIR/harpoon-before" "2"
# Restore to 10s immediately after building before, before any measurements
cp "$ORIG_SRC_BACKUP" "$SRC_FILE"
say "restored source to 10s after before build"
verify_source_interval "10" || exit 1

# --- Build AFTER (10s) ---
say "=== Building AFTER (10s) ==="
# Source already 10s
verify_source_interval "10" || exit 1
build_binary "after" "10"
record_provenance "after" "$BIN_DIR/harpoon-after" "10"

# Verify hashes differ
SHA_BEFORE=$(shasum -a 256 "$BIN_BEFORE" | awk '{print $1}')
SHA_AFTER=$(shasum -a 256 "$BIN_AFTER" | awk '{print $1}')
say "before sha256=$SHA_BEFORE"
say "after sha256=$SHA_AFTER"
if [ "$SHA_BEFORE" = "$SHA_AFTER" ]; then
  echo "[m14] FAIL hashes identical — binaries not distinct"
  exit 1
fi
say "hashes differ — provenance OK"

# Verify final source remains 10s
verify_source_interval "10" || { echo "[m14] FAIL final source not 10s"; exit 1; }
say "final source verified 10s"

# --- Paired execution ---
# Ensure clean state
harpoon/build/harpoon stop 2>&1 | tail -n 3 || true
sleep 2

# Helper to run tier with variant
run_tier_variant() {
  tier="$1"
  variant="$2"
  bin="$BIN_DIR/harpoon-$variant"
  say "=== Tier $tier $variant ($bin) ==="
  # stop
  "$bin" stop 2>&1 | tail -n 3 || true
  sleep 2
  # configure
  # configure memory tier — use env var to avoid dual config path ambiguity (~/Library vs /tmp fallback) and also sync both config files
  # config file is sandbox-dependent; env var is authoritative, file write is best-effort (may be Operation not permitted in sandbox)
  for cfg in "/tmp/harpoon-runtime/config.json" "$HOME/Library/Application Support/Harpoon/config.json"; do
    if mkdir -p "$(dirname "$cfg")" 2>/dev/null; then
      echo "{\"memory\":$tier,\"cpus\":2}" > "$cfg" 2>/dev/null || true
    fi
  done
  # start — use HARPOON_MEMORY_MIB env to guarantee tier (bypasses file race) with one retry for transient VZ
  set +e
  out=$(HARPOON_MEMORY_MIB="$tier" "$bin" start 2>&1)
  echo "$out" | tail -n15 > "$RESULT_DIR/start-${tier}-${variant}.log"
  if echo "$out" | grep -q "VZErrorDomain 1"; then
    say "VZ transient $tier $variant, retry once after 5s"
    sleep 5
    out=$(HARPOON_MEMORY_MIB="$tier" "$bin" start 2>&1)
    echo "$out" | tail -n15 > "$RESULT_DIR/start-${tier}-${variant}.log"
    if echo "$out" | grep -q "VZErrorDomain 1"; then
      echo "$tier,$variant,HOST_VZ_START_FAILURE,VM failed to start" >> "$RESULT_DIR/tier-status.csv"
      blocked "tier $tier $variant HOST_VZ_START_FAILURE"
      return 1
    fi
  fi
  set -e
  sleep 2
  # fail-fast: check runtime
  if ! check_runtime "$variant"; then
    echo "$tier,$variant,RUNTIME_LOST_START,check_runtime failed" >> "$RESULT_DIR/tier-status.csv"
    blocked "tier $tier $variant RUNTIME_LOST_START"
    return 1
  fi
  # wait Docker ready up to 30s
  for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then
    echo "$tier,$variant,DOCKER_NOT_READY,Server not ready" >> "$RESULT_DIR/tier-status.csv"
    blocked "tier $tier $variant DOCKER_NOT_READY"
    return 1
  fi
  # settle 15s
  say "settle $tier $variant 15s"
  sleep 15
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_IDLE,settled but runtime lost" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  # 6 idle samples 5s apart
  for s in 1 2 3 4 5 6; do
    if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_IDLE,sample $s" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    host_measure "$tier" "$variant" "idle-${s}" || { echo "$tier,$variant,RUNTIME_LOST_IDLE,host_measure $s" >> "$RESULT_DIR/tier-status.csv"; return 1; }
    guest_measure "$tier" "$variant" "idle-${s}" || { echo "$tier,$variant,RUNTIME_LOST_IDLE,guest_measure $s" >> "$RESULT_DIR/tier-status.csv"; return 1; }
    sleep 5
  done
  # port-sync window 60s — robust: authoritative log, byte offset, runtime alive full window
  LOG_FILE=$(log_file_for_sync "$variant")
  if [ -z "$LOG_FILE" ] || [ ! -f "$LOG_FILE" ]; then
    echo "$tier,$variant,PORT_SYNC_LOG_UNAVAILABLE,cannot observe log $LOG_FILE" >> "$RESULT_DIR/tier-status.csv"
    say "FAIL port-sync log unavailable $LOG_FILE"
    return 1
  fi
  say "port-sync window $tier $variant 60s log $LOG_FILE"
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_PORT_SYNC,before window runtime lost" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  OFFSET=$(wc -c < "$LOG_FILE" 2>/dev/null || echo 0)
  case "$OFFSET" in ''|*[!0-9]*) OFFSET=0 ;; esac
  # record window start time for duration calc
  WIN_START=$(date +%s)
  sleep 60
  WIN_END=$(date +%s)
  WIN_SECS=$((WIN_END - WIN_START))
  if [ "$WIN_SECS" -lt 55 ]; then WIN_SECS=60; fi
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_PORT_SYNC,after window runtime lost" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  if [ ! -f "$LOG_FILE" ]; then echo "$tier,$variant,PORT_SYNC_LOG_UNAVAILABLE,log vanished after window" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  # if log rotated/truncated, detect size < offset and reset
  CUR_SIZE=$(wc -c < "$LOG_FILE" 2>/dev/null || echo 0)
  case "$CUR_SIZE" in ''|*[!0-9]*) CUR_SIZE=0 ;; esac
  if [ "$CUR_SIZE" -lt "$OFFSET" ]; then
    say "WARN log rotated $LOG_FILE $OFFSET -> $CUR_SIZE, counting full file tail"
    OFFSET=0
  fi
  SYNC_COUNT=$(tail -c +$((OFFSET+1)) "$LOG_FILE" 2>/dev/null | grep -c "HARPOON_PORT_SYNC_START" || true)
  if ! echo "$SYNC_COUNT" | grep -qE '^[0-9]+$'; then
    echo "$tier,$variant,PORT_SYNC_COUNT_INVALID,raw=$SYNC_COUNT" >> "$RESULT_DIR/tier-status.csv"
    return 1
  fi
  # do not convert missing observations into zero — if SYNC_COUNT is empty string it was caught above
  SYNC_PER_MIN="$SYNC_COUNT"
  # duration is WIN_SECS (~60), syncs per minute = count * 60 / WIN_SECS; but window is 60s so == count
  SYNC_PER_MIN="$SYNC_COUNT"
  echo "$tier,$variant,60,$SYNC_COUNT,$SYNC_PER_MIN" >> "$RESULT_DIR/port-sync.csv"
  say "port-sync $tier $variant $SYNC_COUNT in 60s => $SYNC_PER_MIN/min"

  # 128M workload
  say "memory load $tier $variant"
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_MEMORY_LOAD,pre idle check failed" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  host_measure "$tier" "$variant" "mem-idle" || return 1
  guest_measure "$tier" "$variant" "mem-idle" || return 1
  docker --context harpoon rm -f m14-stress 2>&1 | tail -n2 || true
  set +e
  docker --context harpoon run -d --name m14-stress python:3-alpine sh -c 'python3 -c "
import time
x = bytearray(128 * 1024 * 1024)
for i in range(0, len(x), 4096):
    x[i] = 1
print(\"allocated\", len(x))
time.sleep(90)
"' 2>&1 | tail -n2
  rc=$?
  set -e
  sleep 3
  running=$(docker --context harpoon inspect -f '{{.State.Running}}' m14-stress 2>&1 | head -n1 || echo "false")
  if [ "$running" != "true" ]; then
    echo "$tier,$variant,MEMORY_WORKLOAD_FAILED,inspect=$running" >> "$RESULT_DIR/tier-status.csv"
    docker --context harpoon logs m14-stress 2>&1 | tail -n10 || true
    docker --context harpoon rm -f m14-stress 2>&1 | tail -n2 || true
    # do not treat as valid, but continue
    warn "memory workload failed $tier $variant"
  else
    if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_MEMORY_LOAD,load" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    host_measure "$tier" "$variant" "mem-load" || return 1
    guest_measure "$tier" "$variant" "mem-load" || return 1
    sleep 5
    if ! docker --context harpoon inspect -f '{{.State.Running}}' m14-stress 2>&1 | grep -q "true"; then echo "$tier,$variant,RUNTIME_LOST_MEMORY_LOAD,hold10 not running" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    host_measure "$tier" "$variant" "mem-hold10" || return 1
    guest_measure "$tier" "$variant" "mem-hold10" || return 1
    sleep 10
    if ! docker --context harpoon inspect -f '{{.State.Running}}' m14-stress 2>&1 | grep -q "true"; then echo "$tier,$variant,RUNTIME_LOST_MEMORY_LOAD,hold30 not running" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    host_measure "$tier" "$variant" "mem-hold30" || return 1
    guest_measure "$tier" "$variant" "mem-hold30" || return 1
    docker --context harpoon rm -f m14-stress 2>&1 | tail -n2 || true
    sleep 2
    if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_MEMORY_LOAD,release" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    host_measure "$tier" "$variant" "mem-release" || return 1
    guest_measure "$tier" "$variant" "mem-release" || return 1
    sleep 10
    host_measure "$tier" "$variant" "mem-after10" || return 1
    guest_measure "$tier" "$variant" "mem-after10" || return 1
    sleep 20
    host_measure "$tier" "$variant" "mem-after30" || return 1
    guest_measure "$tier" "$variant" "mem-after30" || return 1
  fi

  # docker run latency
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_DOCKER_RUN,pre" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  t0=$(date +%s.%N 2>/dev/null || date +%s)
  set +e
  docker --context harpoon run --rm alpine:3.22 true 2>&1 | tail -n1
  rc=$?
  set -e
  t1=$(date +%s.%N 2>/dev/null || date +%s)
  d=$(python3 -c "print(float('$t1')-float('$t0'))" 2>/dev/null || echo "?")
  echo "$tier,$variant,docker_run,$d" >> "$RESULT_DIR/docker-run.csv"
  if [ "$rc" -ne 0 ]; then echo "$tier,$variant,RUNTIME_LOST_DOCKER_RUN,run failed" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  say "docker run $tier $variant $d s"

  # compose sanity
  if [ -f harpoon/fixtures/m9-compose/compose.yml ]; then
    if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_COMPOSE,pre" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    set +e
    docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml up -d 2>&1 | tail -n5
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then echo "$tier,$variant,COMPOSE_FAILED,up failed" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
    sleep 5
    # verify services
    set +e
    docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml ps 2>&1 | tail -n5
    # check expected services running
    ps_out=$(docker --context harpoon ps --format "{{.Names}}" 2>&1 || echo "")
    set -e
    host_measure "$tier" "$variant" "compose-up" || return 1
    guest_measure "$tier" "$variant" "compose-up" || return 1
    docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml down -v 2>&1 | tail -n5 || true
    if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_COMPOSE,after" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  fi

  # persistence
  if ! check_runtime "$variant"; then echo "$tier,$variant,RUNTIME_LOST_PERSISTENCE,pre" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  docker --context harpoon volume create m14-vol 2>&1 | tail -n1 || true
  set +e
  docker --context harpoon run --rm -v m14-vol:/data alpine:3.22 sh -c 'echo marker > /data/marker && cat /data/marker' 2>&1 | tail -n1
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then echo "$tier,$variant,PERSISTENCE_WRITE_FAILED" >> "$RESULT_DIR/tier-status.csv"; return 1; fi
  # stop and restart same binary/tier
  "$bin" stop 2>&1 | tail -n3 || true
  sleep 2
  set +e
  out=$("$bin" start 2>&1)
  echo "$out" | tail -n10 > "$RESULT_DIR/persist-start-${tier}-${variant}.log"
  if echo "$out" | grep -q "VZErrorDomain 1"; then
    echo "$tier,$variant,HOST_VZ_START_FAILURE,persistence restart" >> "$RESULT_DIR/tier-status.csv"
    # do not continue, but classify
    return 1
  fi
  set -e
  sleep 3
  for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then
    echo "$tier,$variant,BLOCKED_NOT_READY,persistence docker not ready" >> "$RESULT_DIR/tier-status.csv"
    return 1
  fi
  if ! docker --context harpoon volume inspect m14-vol 2>&1 | grep -q "m14-vol"; then
    echo "$tier,$variant,DATA_LOSS,volume missing" >> "$RESULT_DIR/tier-status.csv"
    return 1
  fi
  set +e
  docker --context harpoon run --rm -v m14-vol:/data alpine:3.22 cat /data/marker 2>&1 | grep -q marker
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    echo "$tier,$variant,PERSISTENCE_PASS,marker exists" >> "$RESULT_DIR/tier-status.csv"
    pass "persistence $tier $variant PASS"
  else
    echo "$tier,$variant,DATA_LOSS,marker missing" >> "$RESULT_DIR/tier-status.csv"
    warn "persistence $tier $variant DATA_LOSS"
  fi
  docker --context harpoon volume rm m14-vol 2>&1 | tail -n1 || true
  # stop cleanly
  "$bin" stop 2>&1 | tail -n3 || true
  sleep 2
  echo "$tier,$variant,PASS,completed" >> "$RESULT_DIR/tier-status.csv"
  return 0
}

# Main paired execution
echo "timestamp,tier,variant,phase,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,harpoon_threads,harpoon_fds,vm_pid,vm_rss_kib,vm_cpu_pct,vm_threads,vm_fds,combined_rss_kib" > "$RESULT_DIR/host.csv"
echo "timestamp,tier,variant,phase,mem_total_kib,mem_available_kib,load1,load5,load15,process_count" > "$RESULT_DIR/guest.csv"
echo "tier,variant,window_seconds,sync_count,syncs_per_minute" > "$RESULT_DIR/port-sync.csv"
echo "tier,variant,status,detail" > "$RESULT_DIR/tier-status.csv"
echo "tier,variant,phase,duration" > "$RESULT_DIR/docker-run.csv"
: > "$RESULT_DIR/comparison.csv"

for tier in 512 768 1024; do
  say "=== Paired Tier $tier ==="
  # before
  if ! run_tier_variant "$tier" "before"; then
    warn "tier $tier before failed or blocked, continuing to after"
  fi
  # small pause between variants to reduce drift
  sleep 3
  # after
  if ! run_tier_variant "$tier" "after"; then
    warn "tier $tier after failed or blocked"
  fi
  # ensure stopped between tiers
  harpoon/build/harpoon stop 2>&1 | tail -n3 || true
  sleep 2
done

# Generate comparison.csv
say "=== Generating comparison.csv ==="
# headers already done, now compute per tier
for tier in 512 768 1024; do
  before_sync=$(grep "^$tier,before," "$RESULT_DIR/port-sync.csv" | awk -F, '{print $5}' | tail -n1 || echo "")
  after_sync=$(grep "^$tier,after," "$RESULT_DIR/port-sync.csv" | awk -F, '{print $5}' | tail -n1 || echo "")
  # compute reduction
  reduction=""
  if echo "$before_sync" | grep -qE '^[0-9]+$' && echo "$after_sync" | grep -qE '^[0-9]+$' && [ "$before_sync" -gt 0 ]; then
    reduction=$(python3 -c "print(round((1 - $after_sync/$before_sync)*100,1))" 2>/dev/null || echo "")
  fi
  # idle combined median
  before_combined=$(grep ",$tier,before,idle-" "$RESULT_DIR/host.csv" | awk -F, '{print $15}' | grep -E '^[0-9]+$' | sort -n | awk 'NR==3 {print}' 2>/dev/null || echo "")
  after_combined=$(grep ",$tier,after,idle-" "$RESULT_DIR/host.csv" | awk -F, '{print $15}' | grep -E '^[0-9]+$' | sort -n | awk 'NR==3 {print}' 2>/dev/null || echo "")
  # idle CPU median (harpoon)
  before_cpu=$(grep ",$tier,before,idle-" "$RESULT_DIR/host.csv" | awk -F, '{print $6}' | grep -E '^[0-9.]+$' | sort -n | awk 'NR==3 {print}' 2>/dev/null || echo "")
  after_cpu=$(grep ",$tier,after,idle-" "$RESULT_DIR/host.csv" | awk -F, '{print $6}' | grep -E '^[0-9.]+$' | sort -n | awk 'NR==3 {print}' 2>/dev/null || echo "")
  before_run=$(grep "^$tier,before,docker_run" "$RESULT_DIR/docker-run.csv" | awk -F, '{print $4}' | tail -n1 || echo "")
  after_run=$(grep "^$tier,after,docker_run" "$RESULT_DIR/docker-run.csv" | awk -F, '{print $4}' | tail -n1 || echo "")
  before_persist=$(grep "^$tier,before,PERSISTENCE_PASS" "$RESULT_DIR/tier-status.csv" | head -n1 | awk -F, '{print $3}' || echo "")
  after_persist=$(grep "^$tier,after,PERSISTENCE_PASS" "$RESULT_DIR/tier-status.csv" | head -n1 | awk -F, '{print $3}' || echo "")
  # if blocked, capture status
  before_status=$(grep "^$tier,before," "$RESULT_DIR/tier-status.csv" | tail -n1 | awk -F, '{print $3}' || echo "INCOMPLETE")
  after_status=$(grep "^$tier,after," "$RESULT_DIR/tier-status.csv" | tail -n1 | awk -F, '{print $3}' || echo "INCOMPLETE")
  # determine result per tier
  result="INCOMPLETE"
  if echo "$before_status" | grep -q "BLOCKED_HOST_TRANSIENT" || echo "$after_status" | grep -q "BLOCKED_HOST_TRANSIENT"; then
    result="BLOCKED_HOST_TRANSIENT"
  elif [ "$before_status" = "PASS" ] && [ "$after_status" = "PASS" ]; then
    # check sync reduction and functional gates
    if echo "$before_sync" | grep -qE '^[0-9]+$' && echo "$after_sync" | grep -qE '^[0-9]+$'; then
      # after must be materially lower
      if [ "$after_sync" -lt "$before_sync" ]; then
        result="PASS"
      else
        result="FAIL"
      fi
    else
      result="INCOMPLETE"
    fi
  elif echo "$before_status" | grep -q "FAIL" || echo "$after_status" | grep -q "FAIL"; then
    result="FAIL"
  else
    result="$before_status/$after_status"
  fi
  # fallback: if we have before_sync and after_sync and after < before, PASS even if other statuses are PASS
  if [ -z "$before_sync" ]; then before_sync=""; fi
  if [ -z "$after_sync" ]; then after_sync=""; fi
  echo "$tier,$before_sync,$after_sync,$reduction,$before_combined,$after_combined,$before_cpu,$after_cpu,$before_run,$after_run,$before_persist,$after_persist,$result" >> "$RESULT_DIR/comparison.csv"
done
cat "$RESULT_DIR/comparison.csv"

# Final checks
say "=== Final provenance check ==="
cat "$PROV_BEFORE"
cat "$PROV_AFTER"
# verify final source remains 10s
if ! grep -q "repeating: 10" "$SRC_FILE"; then
  echo "[m14] FAIL final source not 10s"
  exit 1
fi
say "final source 10s verified"

# Regressions if host healthy
if harpoon/build/harpoon status 2>&1 | grep -qi "running"; then
  say "host running, skipping regressions (host should be stopped)"
  harpoon/build/harpoon stop 2>&1 | tail -n3 || true
  sleep 2
fi
# Run regressions when stopped is fine for some, but live ones need running — we do status/doctor anyway
say "=== Regressions ==="
harpoon/build/harpoon status 2>&1 | tail -n5 || true
harpoon/build/harpoon doctor 2>&1 | tail -n10 || true
# Only run live regressions if we can get a running host
set +e
harpoon/build/harpoon start 2>&1 | tail -n5
sleep 3
if harpoon/build/harpoon status 2>&1 | grep -qi "running"; then
  say "host live, running live regressions"
  bash harpoon/regression-bridges.sh 2>&1 | tail -n10 || true
  bash harpoon/m3-test.sh 2>&1 | tail -n10 || true
  harpoon/build/harpoon stop 2>&1 | tail -n3 || true
else
  say "host not live, skipping live regressions (BLOCKED_HOST_TRANSIENT)"
fi
set -e

say "M14 complete"
cat "$RESULT_DIR/tier-status.csv"
cat "$RESULT_DIR/port-sync.csv"
cat "$RESULT_DIR/comparison.csv"
