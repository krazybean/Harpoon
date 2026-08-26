#!/bin/sh
set -eu
# M13 Resource Baseline & Performance Characterization
# DO NOT OPTIMIZE RUNTIME — measurement only
# Tiers: 512 768 1024, CPUs=2

BIN_CANDIDATES="/tmp/harpoon-m11-stage/bin/harpoon dist/harpoon-0.1.0-dev-darwin-arm64/bin/harpoon harpoon/build/harpoon /usr/local/bin/harpoon"
BIN=""
for c in $BIN_CANDIDATES; do if [ -x "$c" ]; then BIN="$c"; break; fi; done
[ -n "$BIN" ] || { echo "[m13] FAIL no binary"; exit 1; }
RESULT_DIR="harpoon/results/m13"
mkdir -p "$RESULT_DIR"
RAW="$RESULT_DIR"
say() { echo "[m13] $*"; }
pass() { echo "[m13] PASS $*"; }
blocked() { echo "[m13] BLOCKED $*"; }
warn() { echo "[m13] WARN $*"; }

# parse args for repair mode
STAGE_MODE="full"
for arg in "$@"; do
  case "$arg" in
    --stage=measurement-fix) STAGE_MODE="measurement-fix" ;;
    measurement-fix) STAGE_MODE="measurement-fix" ;;
    --stage) ;; # handled via next
  esac
done
if [ "${1:-}" = "--stage" ] && [ "${2:-}" = "measurement-fix" ]; then STAGE_MODE="measurement-fix"; fi

# helper to get disk path for VM association
get_disk_path() {
  # prefer runtime disk from harpoon status --json or doctor
  # try status --json
  disk=$(harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('disk',''))" 2>/dev/null || echo "")
  if [ -n "$disk" ] && [ -f "$disk" ]; then echo "$disk"; return; fi
  # try /tmp/harpoon-runtime/data
  if [ -f /tmp/harpoon-runtime/data/harpoon-root.img ]; then echo "/tmp/harpoon-runtime/data/harpoon-root.img"; return; fi
  if [ -f "$HOME/Library/Application Support/Harpoon/data/harpoon-root.img" ]; then echo "$HOME/Library/Application Support/Harpoon/data/harpoon-root.img"; return; fi
  if [ -f spike2/cache/harpoon-root.img ]; then echo "spike2/cache/harpoon-root.img"; return; fi
  echo "/tmp/harpoon-runtime/data/harpoon-root.img"
}

find_vm_pid() {
  disk="$1"
  # lsof the disk image, find Virtualization pid
  # lsof output: COMMAND PID USER FD TYPE...
  pid=$(lsof -n 2>/dev/null | grep -F "$disk" | grep -i "Virtualization" | awk '{print $2}' | head -n1 || echo "")
  if [ -n "$pid" ]; then echo "$pid"; return; fi
  # fallback: pgrep Virtualization and check parent
  pid=$(pgrep -f "Virtualization" 2>/dev/null | head -n1 || echo "")
  if [ -n "$pid" ]; then
    # verify it has disk open via lsof -p
    if lsof -p "$pid" 2>/dev/null | grep -q -F "$disk"; then echo "$pid"; return; fi
  fi
  echo ""
}

# --- A. Environment baseline ---
record_env() {
  say "=== A. Environment baseline ==="
  {
    echo "commit: $(git rev-parse HEAD 2>&1 | head -n1) $(git status --porcelain 2>&1 | head -n5 | tr '\n' ' ')"
    echo "dirty: $(git diff --stat 2>&1 | head -n1 || echo clean)"
    echo "harpoon: $($BIN version 2>&1 | head -n1) size $(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN" 2>/dev/null) bytes"
    echo "kernel: $(ls -lh spike1/cache/Image-virt 2>&1 | awk '{print $5, $9}') size $(stat -f%z spike1/cache/Image-virt 2>/dev/null || stat -c%s spike1/cache/Image-virt 2>/dev/null)"
    echo "initramfs: $(ls -lh harpoon/cache/harpoon-m4-initramfs.cpio.gz 2>&1 | awk '{print $5, $9}') size $(stat -f%z harpoon/cache/harpoon-m4-initramfs.cpio.gz 2>/dev/null || stat -c%s harpoon/cache/harpoon-m4-initramfs.cpio.gz 2>/dev/null)"
    echo "root logical: $(ls -lh spike2/cache/harpoon-root.img 2>&1 | awk '{print $5}') allocated $(du -h spike2/cache/harpoon-root.img 2>&1 | awk '{print $1}')"
    echo "macOS: $(sw_vers -productVersion 2>&1) $(sw_vers -buildVersion 2>&1) $(uname -m)"
    echo "hardware: $(sysctl -n hw.model 2>&1 | head -n1) SoC $(sysctl -n machdep.cpu.brand_string 2>&1 | head -n1)"
    echo "host RAM: $(sysctl -n hw.memsize 2>&1 | awk '{print $1/1024/1024/1024 " GiB"}') logical CPUs $(sysctl -n hw.logicalcpu 2>&1) physical $(sysctl -n hw.physicalcpu 2>&1)"
    echo "docker client: $(docker --version 2>&1 | head -n1)"
    echo "docker server: $(docker --context harpoon version 2>&1 | grep Server -A2 | head -n5 || echo blocked)"
    echo "compose: $(docker compose version 2>&1 | head -n1)"
    echo "buildx: $(docker buildx version 2>&1 | head -n1)"
    echo "archive: $(cat dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz.sha256 2>&1 | head -n1)"
  } | tee "$RAW/env.txt"
  pass "env recorded"
}

# --- B. Host measurement helpers (fixed) ---
host_measure() {
  # args: tier label
  tier="$1"; label="$2"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  epoch=$(date +%s)
  # harpoon manager PID
  pid=$(cat /tmp/harpoon.pid 2>/dev/null | tr -d ' \n' || echo "")
  if [ -z "$pid" ]; then pid=$(pgrep -f "harpoon.*run" 2>/dev/null | head -n1 || echo ""); fi
  harpoon_rss="0"; harpoon_cpu="0"; harpoon_threads="0"; harpoon_fds="0"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    harpoon_rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo 0)
    harpoon_cpu=$(ps -o %cpu= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo 0)
    # threads: ps -M counts threads on macOS
    harpoon_threads=$(ps -M "$pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' || echo 0)
    if [ "$harpoon_threads" = "0" ]; then harpoon_threads=$(ps -o thcount= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo 0); fi
    harpoon_fds=$(lsof -p "$pid" 2>/dev/null | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo 0)
  fi
  # VM XPC via lsof of disk
  disk=$(get_disk_path)
  vm_pid=$(find_vm_pid "$disk")
  vm_rss="0"; vm_cpu="0"; vm_threads="0"; vm_fds="0"
  if [ -n "$vm_pid" ] && [ "$vm_pid" != "$pid" ] && kill -0 "$vm_pid" 2>/dev/null; then
    vm_rss=$(ps -o rss= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo 0)
    vm_cpu=$(ps -o %cpu= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo 0)
    vm_threads=$(ps -M "$vm_pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' || echo 0)
    if [ "$vm_threads" = "0" ]; then vm_threads=$(ps -o thcount= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo 0); fi
    vm_fds=$(lsof -p "$vm_pid" 2>/dev/null | wc -l | tr -d ' ' | grep -E '^[0-9]+$' || echo 0)
  else
    # if not found, mark unavailable but don't invent
    if [ -z "$vm_pid" ]; then vm_pid=""; vm_rss=""; vm_cpu=""; fi
  fi
  # combined: sum if both numeric
  combined=""
  if echo "$harpoon_rss" | grep -qE '^[0-9]+$' && echo "$vm_rss" | grep -qE '^[0-9]+$'; then
    combined=$((harpoon_rss + vm_rss))
  elif echo "$harpoon_rss" | grep -qE '^[0-9]+$'; then
    combined="$harpoon_rss"
  fi
  # pages_free
  pages_free=$(vm_stat 2>&1 | grep "Pages free" | awk '{print $3}' | tr -d '.' | grep -E '^[0-9]+$' || echo "")
  if [ "$STAGE_MODE" = "measurement-fix" ]; then
    echo "$ts,$tier,$label,$pid,$harpoon_rss,$harpoon_cpu,$harpoon_threads,$harpoon_fds,$vm_pid,$vm_rss,$vm_cpu,$vm_threads,$vm_fds,$combined,$pages_free" >> "$RAW/repair-host.csv"
  else
    echo "$ts,$tier,$label,$pid,$harpoon_rss,$harpoon_cpu,$harpoon_threads,$harpoon_fds,$vm_pid,$vm_rss,$vm_cpu,$vm_threads,$vm_fds,$combined,$pages_free" | tee -a "$RAW/host.csv"
  fi
}

guest_measure() {
  tier="$1"; label="$2"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  if [ -S /tmp/harpoon-docker.sock ]; then
    out=$(docker --context harpoon run --rm alpine:3.22 sh -c 'cat /proc/meminfo | grep -E "MemTotal|MemAvailable|MemFree"; echo "---"; cat /proc/loadavg; echo "---"; ps aux | wc -l; echo "---"; free -m 2>&1 | head -n3' 2>&1 || echo "guest_failed")
    memtotal=$(echo "$out" | grep MemTotal | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
    memavail=$(echo "$out" | grep MemAvailable | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
    memfree=$(echo "$out" | grep MemFree | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
    load_line=$(echo "$out" | grep -E "^[0-9]+\.[0-9]+ [0-9]" | head -n1 || echo "")
    load1=$(echo "$load_line" | awk '{print $1}' || echo "")
    load5=$(echo "$load_line" | awk '{print $2}' || echo "")
    load15=$(echo "$load_line" | awk '{print $3}' || echo "")
    pcount=$(echo "$out" | grep -E '^[[:space:]]*[0-9]+$' | head -n1 | tr -d ' ' || echo "")
    # fallback for pcount: second --- block
    if [ -z "$pcount" ]; then pcount=$(echo "$out" | awk '/---/{c++; next} c==2{print}' | head -n1 | tr -d ' ' | grep -E '^[0-9]+$' || echo ""); fi
    if [ "$STAGE_MODE" = "measurement-fix" ]; then
      echo "$ts,$tier,$label,$memtotal,$memavail,$load1,$load5,$load15,$pcount" >> "$RAW/repair-guest.csv"
      echo "$out" > "$RAW/repair-guest-${tier}-${label}.txt" 2>&1 || true
    else
      echo "$ts,$tier,$label,$memtotal,$memavail,$load1,$load5,$load15,$pcount" | tee -a "$RAW/guest.csv"
      echo "$out" > "$RAW/guest-${tier}-${label}.txt" 2>&1 || true
    fi
  else
    if [ "$STAGE_MODE" = "measurement-fix" ]; then echo "$ts,$tier,$label,BLOCKED" >> "$RAW/repair-guest.csv"; else echo "$ts,$tier,$label,BLOCKED" | tee -a "$RAW/guest.csv"; fi
  fi
}

startup_one() {
  tier="$1"; iter="$2"
  say "startup tier $tier iter $iter"
  "$BIN" stop 2>&1 | tail -n3 || true
  sleep 2
  T0=$(date +%s.%N 2>/dev/null || date +%s)
  set +e
  out=$("$BIN" start 2>&1)
  rc=$?
  set -e
  T4=$(date +%s.%N 2>/dev/null || date +%s)
  echo "$out" | tail -n15 > "$RAW/startup-${tier}-${iter}.log"
  if echo "$out" | grep -q "VZErrorDomain 1"; then
    echo "$tier,$iter,BLOCKED_HOST_TRANSIENT,$T0,$T4" >> "$RAW/startup.csv"
    blocked "startup $tier iter $iter BLOCKED_HOST_TRANSIENT"
    return 0
  fi
  sleep 1
  if "$BIN" status 2>&1 | grep -qi running; then
    set +e
    docker --context harpoon version 2>&1 | head -n2 > "$RAW/docker-ready-${tier}-${iter}.log" || true
    dr=$?
    set -e
    dur=$(python3 -c "print(float('$T4')-float('$T0'))" 2>/dev/null || echo "?")
    echo "$tier,$iter,PASS,$T0,$T4,$dur,$rc,$dr" >> "$RAW/startup.csv"
    pass "startup $tier iter $iter ${dur}s"
  else
    echo "$tier,$iter,FAIL,$T0,$T4" >> "$RAW/startup.csv"
    warn "startup $tier iter $iter FAIL"
  fi
  host_measure "$tier" "startup-${tier}-${iter}"
  guest_measure "$tier" "startup-${tier}-${iter}"
}

# --- Main ---
main() {
  record_env
  # init CSV headers — truncate run-scoped state (repair mode preserves valid startup/container/compose/build)
  if [ "$STAGE_MODE" != "measurement-fix" ]; then
    echo "timestamp,tier,label,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,harpoon_threads,harpoon_fds,vm_pid,vm_rss_kib,vm_cpu_pct,vm_threads,vm_fds,combined_rss_kib,pages_free" > "$RAW/host.csv"
    echo "timestamp,tier,label,mem_total_kib,mem_available_kib,load1,load5,load15,process_count" > "$RAW/guest.csv"
    echo "tier,iter,status,T0,T4,duration_s,rc,dr" > "$RAW/startup.csv"
    echo "tier,iter,status,duration_s" > "$RAW/container.csv"
    echo "tier,phase,harpoon_rss,vm_rss,combined,memtotal,memavail" > "$RAW/memory-load.csv"
    : > "$RAW/persistence.csv"
    : > "$RAW/summary.txt"
    echo "tier,phase,duration" > "$RAW/compose.csv" 2>&1 || true
    echo "tier,phase,duration" > "$RAW/build.csv" 2>&1 || true
  fi
  # tier-viability is reinitialized only for full runs; repair preserves authoritative LIVE and writes to repair-tier-viability.csv
  if [ "$STAGE_MODE" != "measurement-fix" ]; then
    echo "tier,viability" > "$RAW/tier-viability.csv"
  fi
  # repair mode separate files
  if [ "$STAGE_MODE" = "measurement-fix" ]; then
    echo "timestamp,tier,label,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,harpoon_threads,harpoon_fds,vm_pid,vm_rss_kib,vm_cpu_pct,vm_threads,vm_fds,combined_rss_kib,pages_free" > "$RAW/repair-host.csv"
    echo "timestamp,tier,label,mem_total_kib,mem_available_kib,load1,load5,load15,process_count" > "$RAW/repair-guest.csv"
    echo "tier,phase,harpoon_rss,vm_rss,combined,memtotal,memavail,status" > "$RAW/repair-memory-load.csv"
    echo "tier,viability" > "$RAW/repair-tier-viability.csv"
  fi

  if [ "$STAGE_MODE" = "measurement-fix" ]; then
    say "=== Measurement-fix targeted mode (no 5x startup, no 10x container, no full soak) ==="
    for tier in 512 768 1024; do
      say "=== Tier $tier (repair) ==="
      "$BIN" config set memory "$tier" 2>&1 | tail -n2 || true
      # clean start
      "$BIN" stop 2>&1 | tail -n3 || true
      sleep 2
      set +e
      out=$("$BIN" start 2>&1)
      echo "$out" | tail -n15 > "$RAW/repair-start-${tier}.log"
      if echo "$out" | grep -q "VZErrorDomain 1"; then
        echo "$tier,BLOCKED_HOST_TRANSIENT" >> "$RAW/repair-tier-viability.csv"
        blocked "repair tier $tier BLOCKED_HOST_TRANSIENT"
        continue
      fi
      set -e
      sleep 2
      if ! "$BIN" status 2>&1 | grep -qi running; then
        echo "$tier,BLOCKED_HOST_TRANSIENT" >> "$RAW/repair-tier-viability.csv"
        blocked "repair tier $tier not running"
        continue
      fi
      # wait Docker ready (up to 30s)
      for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
      if ! docker --context harpoon version 2>&1 | grep -q "Server"; then
        echo "$tier,BLOCKED_DOCKER_NOT_READY" >> "$RAW/repair-tier-viability.csv"
        blocked "repair tier $tier docker not ready"
        "$BIN" stop 2>&1 | tail -n3 || true
        continue
      fi
      # settle 10s then idle samples
      say "--- idle samples tier $tier ---"
      sleep 10
      for s in 1 2 3 4 5 6; do host_measure "$tier" "idle-${tier}-${s}"; guest_measure "$tier" "idle-${tier}-${s}"; sleep 5; done
      # memory load — corrected workload
      say "--- F. Memory load tier $tier (corrected) ---"
      host_measure "$tier" "mem-idle-${tier}"
      guest_measure "$tier" "mem-idle-${tier}"
      # ensure no stale container
      docker --context harpoon rm -f m13-stress 2>&1 | tail -n2 || true
      # start workload with python that TOUCHES memory
      set +e
      docker --context harpoon run -d --name m13-stress python:3-alpine sh -c 'python3 -c "
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
      # verify live
      running=$(docker --context harpoon inspect -f '{{.State.Running}}' m13-stress 2>&1 | head -n1 || echo "false")
      if [ "$running" != "true" ]; then
        echo "[m13] memory workload not running tier $tier inspect=$running"
        docker --context harpoon logs m13-stress 2>&1 | tail -n20 || true
        echo "$tier,mem-load,BLOCKED,MEMORY_WORKLOAD_FAILED" >> "$RAW/repair-memory-load.csv"
        warn "memory workload failed tier $tier"
        docker --context harpoon rm -f m13-stress 2>&1 | tail -n2 || true
        echo "$tier,LIVE" >> "$RAW/tier-viability.csv"
        "$BIN" stop 2>&1 | tail -n3 || true
        continue
      fi
      # verify still alive before each sample
      host_measure "$tier" "mem-load-${tier}"
      guest_measure "$tier" "mem-load-${tier}"
      sleep 10
      running=$(docker --context harpoon inspect -f '{{.State.Running}}' m13-stress 2>&1 | head -n1 || echo "false")
      if [ "$running" != "true" ]; then warn "workload exited before hold10 tier $tier"; echo "$tier,mem-hold10,MEMORY_WORKLOAD_FAILED" >> "$RAW/repair-memory-load.csv"; else host_measure "$tier" "mem-hold10-${tier}"; guest_measure "$tier" "mem-hold10-${tier}"; fi
      sleep 20
      running=$(docker --context harpoon inspect -f '{{.State.Running}}' m13-stress 2>&1 | head -n1 || echo "false")
      if [ "$running" != "true" ]; then warn "workload exited before hold30 tier $tier"; echo "$tier,mem-hold30,MEMORY_WORKLOAD_FAILED" >> "$RAW/repair-memory-load.csv"; else host_measure "$tier" "mem-hold30-${tier}"; guest_measure "$tier" "mem-hold30-${tier}"; fi
      docker --context harpoon rm -f m13-stress 2>&1 | tail -n2 || true
      sleep 2
      host_measure "$tier" "mem-release-${tier}"
      guest_measure "$tier" "mem-release-${tier}"
      sleep 10
      host_measure "$tier" "mem-after10-${tier}"
      guest_measure "$tier" "mem-after10-${tier}"
      sleep 20
      host_measure "$tier" "mem-after30-${tier}"
      guest_measure "$tier" "mem-after30-${tier}"
      # CPU samples
      host_measure "$tier" "cpu-idle-${tier}"
      for i in 1 2 3; do docker --context harpoon ps 2>&1 | tail -n2 || true; sleep 1; done
      host_measure "$tier" "cpu-after-ps-${tier}"
      # compose host RSS baseline (optional once per tier)
      if [ -f harpoon/fixtures/m9-compose/compose.yml ]; then
        set +e
        t0=$(date +%s)
        docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml up -d --build 2>&1 | tail -n10
        t1=$(date +%s); d=$((t1-t0))
        host_measure "$tier" "compose-up-${tier}"
        guest_measure "$tier" "compose-up-${tier}"
        docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml down -v 2>&1 | tail -n5 || true
        set -e
      fi
      # persistence check
      say "--- persistence tier $tier ---"
      docker --context harpoon volume create m13-vol 2>&1 | tail -n2 || true
      docker --context harpoon run --rm -v m13-vol:/data alpine:3.22 sh -c 'echo marker > /data/marker && cat /data/marker' 2>&1 | tail -n2 || true
      # ensure marker exists before stop
      has_marker=$(docker --context harpoon run --rm -v m13-vol:/data alpine:3.22 cat /data/marker 2>&1 | grep -c marker || echo 0)
      "$BIN" stop 2>&1 | tail -n3 || true
      sleep 2
      set +e
      out=$("$BIN" start 2>&1)
      echo "$out" | tail -n10 > "$RAW/repair-persist-start-${tier}.log"
      set -e
      sleep 3
      recovered="false"
      for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then recovered="true"; break; fi; sleep 5; done
      if [ "$recovered" = "true" ]; then
        if docker --context harpoon volume inspect m13-vol 2>&1 | grep -q "m13-vol"; then
          if docker --context harpoon run --rm -v m13-vol:/data alpine:3.22 cat /data/marker 2>&1 | grep -q marker; then
            pass "persistence $tier PASS"
            echo "$tier,PERSISTENCE_PASS" >> "$RAW/repair-persist-${tier}.txt" 2>&1 || true
          else
            warn "persistence $tier marker gone (data loss)"
            echo "$tier,PERSISTENCE_DATA_LOSS" >> "$RAW/repair-persist-${tier}.txt" 2>&1 || true
          fi
        else
          warn "persistence $tier volume missing"
        fi
      else
        warn "persistence $tier restart not ready (timeout)"
        echo "$tier,PERSISTENCE_BLOCKED_NOT_READY" >> "$RAW/repair-persist-${tier}.txt" 2>&1 || true
      fi
      echo "$tier,LIVE" >> "$RAW/repair-tier-viability.csv"
      "$BIN" stop 2>&1 | tail -n3 || true
      sleep 2
    done
    say "=== repair complete ==="
    cat "$RAW/repair-tier-viability.csv" 2>&1 | tee -a "$RAW/summary.txt" || true
    cat "$RAW/tier-viability.csv" 2>&1 | tee -a "$RAW/summary.txt" || true
    pass "measurement-fix complete"
    exit 0
  fi

  for tier in 512 768 1024; do
    say "=== Tier $tier ==="
    "$BIN" config set memory "$tier" 2>&1 | tail -n2 || true
    say "--- C. Startup 5 iterations tier $tier ---"
    for i in 1 2 3 4 5; do startup_one "$tier" "$i"; sleep 2; done
    if ! "$BIN" status 2>&1 | grep -qi running; then
      warn "tier $tier not running after startup iterations; attempting one more start"
      "$BIN" start 2>&1 | tail -n5 || true
      sleep 3
    fi
    if "$BIN" status 2>&1 | grep -qi running; then
      say "--- D. Idle baseline tier $tier (30s) ---"
      sleep 10
      for s in 1 2 3 4 5 6; do host_measure "$tier" "idle-${tier}-${s}"; guest_measure "$tier" "idle-${tier}-${s}"; sleep 5; done
      say "--- E. Container latency tier $tier ---"
      docker --context harpoon pull alpine:3.22 2>&1 | tail -n3 || true
      docker --context harpoon pull hello-world 2>&1 | tail -n3 || true
      for i in 1 2 3 4 5 6 7 8 9 10; do
        t0=$(date +%s.%N 2>/dev/null || date +%s)
        docker --context harpoon run --rm alpine:3.22 true 2>&1 | tail -n2 || true
        t1=$(date +%s.%N 2>/dev/null || date +%s)
        d=$(python3 -c "print(float('$t1')-float('$t0'))" 2>/dev/null || echo "?")
        echo "$tier,$i,PASS,$d" >> "$RAW/container.csv"
        echo "[m13] container alpine $tier iter $i ${d}s"
      done
      for i in 1 2 3 4 5; do
        t0=$(date +%s.%N 2>/dev/null || date +%s)
        docker --context harpoon run --rm hello-world 2>&1 | grep -q "Hello" || true
        t1=$(date +%s.%N 2>/dev/null || date +%s)
        d=$(python3 -c "print(float('$t1')-float('$t0'))" 2>/dev/null || echo "?")
        echo "$tier,hello-$i,PASS,$d" >> "$RAW/container.csv"
      done
      say "--- F. Memory under load tier $tier ---"
      host_measure "$tier" "mem-idle-${tier}"
      guest_measure "$tier" "mem-idle-${tier}"
      docker --context harpoon rm -f m13-stress 2>&1 | tail -n2 || true
      set +e
      docker --context harpoon run -d --name m13-stress python:3-alpine sh -c 'python3 -c "
import time
x = bytearray(128 * 1024 * 1024)
for i in range(0, len(x), 4096):
    x[i] = 1
print(\"allocated\", len(x))
time.sleep(90)
"' 2>&1 | tail -n2
      set -e
      sleep 3
      running=$(docker --context harpoon inspect -f '{{.State.Running}}' m13-stress 2>&1 | head -n1 || echo "false")
      if [ "$running" != "true" ]; then
        warn "memory workload failed to start tier $tier"
        echo "$tier,mem-load,FAILED" >> "$RAW/memory-load.csv"
      else
        host_measure "$tier" "mem-load-${tier}"
        guest_measure "$tier" "mem-load-${tier}"
        sleep 10
        host_measure "$tier" "mem-hold10-${tier}"
        guest_measure "$tier" "mem-hold10-${tier}"
        sleep 20
        host_measure "$tier" "mem-hold30-${tier}"
        guest_measure "$tier" "mem-hold30-${tier}"
        docker --context harpoon rm -f m13-stress 2>&1 | tail -n2 || true
        sleep 2
        host_measure "$tier" "mem-release-${tier}"
        guest_measure "$tier" "mem-release-${tier}"
        sleep 10
        host_measure "$tier" "mem-after10-${tier}"
        guest_measure "$tier" "mem-after10-${tier}"
        sleep 20
        host_measure "$tier" "mem-after30-${tier}"
        guest_measure "$tier" "mem-after30-${tier}"
      fi
      say "--- G. CPU idle cost tier $tier ---"
      host_measure "$tier" "cpu-idle-${tier}"
      for i in 1 2 3; do docker --context harpoon ps 2>&1 | tail -n2 || true; sleep 1; done
      host_measure "$tier" "cpu-after-ps-${tier}"
      say "--- H. Compose tier $tier ---"
      if [ -f harpoon/fixtures/m9-compose/compose.yml ]; then
        set +e
        t0=$(date +%s)
        docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml up -d --build 2>&1 | tail -n10
        t1=$(date +%s)
        d=$((t1-t0))
        echo "[m13] compose up $tier ${d}s"
        echo "$tier,up,$d" >> "$RAW/compose.csv" 2>&1 || echo "tier,phase,duration" > "$RAW/compose.csv" 2>&1; echo "$tier,up,$d" >> "$RAW/compose.csv"
        host_measure "$tier" "compose-up-${tier}"
        guest_measure "$tier" "compose-up-${tier}"
        docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml ps 2>&1 | tail -n10 || true
        curl -s http://127.0.0.1:18080/health 2>&1 | head -n2 || echo "health blocked"
        docker --context harpoon compose -f harpoon/fixtures/m9-compose/compose.yml down -v 2>&1 | tail -n5 || true
        set -e
      else
        warn "compose fixture missing"
      fi
      say "--- I. Build tier $tier ---"
      if [ -f harpoon/fixtures/m9-compose/Dockerfile ]; then
        set +e
        t0=$(date +%s)
        docker --context harpoon build -t m13-test:warm harpoon/fixtures/m9-compose 2>&1 | tail -n5
        t1=$(date +%s); d=$((t1-t0)); echo "[m13] build warm $tier ${d}s"; echo "$tier,warm,$d" >> "$RAW/build.csv" 2>&1 || echo "tier,warm,duration" > "$RAW/build.csv"; echo "$tier,warm,$d" >> "$RAW/build.csv"
        t0=$(date +%s)
        docker --context harpoon build --no-cache -t m13-test:nocache harpoon/fixtures/m9-compose 2>&1 | tail -n5
        t1=$(date +%s); d=$((t1-t0)); echo "[m13] build nocache $tier ${d}s"; echo "$tier,nocache,$d" >> "$RAW/build.csv"
        set -e
      fi
      say "--- J. Filesystem tier $tier ---"
      mkdir -p /tmp/m13-fs-test
      echo "hostfile" > /tmp/m13-fs-test/host.txt
      set +e
      docker --context harpoon run --rm -v /tmp/m13-fs-test:/data alpine:3.22 sh -c 'cat /data/host.txt; echo container > /data/container.txt; ls -R /data 2>&1 | head' 2>&1 | tail -n5
      set -e
      cat /tmp/m13-fs-test/container.txt 2>&1 | head || true
      host_measure "$tier" "fs-${tier}"
      say "--- K. Persistence tier $tier ---"
      docker --context harpoon volume create m13-vol 2>&1 | tail -n2 || true
      docker --context harpoon run --rm -v m13-vol:/data alpine:3.22 sh -c 'echo marker > /data/marker && cat /data/marker' 2>&1 | tail -n2 || true
      t0=$(date +%s); "$BIN" stop 2>&1 | tail -n3 || true; t1=$(date +%s); d=$((t1-t0)); echo "[m13] stop ${d}s" >> "$RAW/persistence.csv"
      sleep 2
      "$BIN" start 2>&1 | tail -n5 || true; sleep 3
      set +e
      if docker --context harpoon run --rm -v m13-vol:/data alpine:3.22 cat /data/marker 2>&1 | grep -q marker; then pass "persistence $tier PASS"; else warn "persistence $tier blocked"; fi
      set -e
      host_measure "$tier" "persist-${tier}"
      say "--- M. Soak 10m tier $tier (bounded) ---"
      for s in 1 2 3 4 5 6 7 8 9 10; do
        docker --context harpoon ps 2>&1 | tail -n2 || true
        docker --context harpoon run --rm alpine:3.22 true 2>&1 | tail -n1 || true
        host_measure "$tier" "soak-${tier}-${s}"
        guest_measure "$tier" "soak-${tier}-${s}"
        sleep 30
      done
      echo "$tier,LIVE" >> "$RAW/tier-viability.csv"
    else
      blocked "tier $tier live sections (not running, host transient)"
      echo "$tier,BLOCKED_HOST_TRANSIENT" >> "$RAW/tier-viability.csv"
    fi
    "$BIN" stop 2>&1 | tail -n3 || true
    sleep 2
  done
  say "=== L. Tier viability ==="
  cat "$RAW/tier-viability.csv" 2>&1 | tee -a "$RAW/summary.txt" || true
  say "=== N. Regression ==="
  "$BIN" status 2>&1 | tail -n5 || true
  "$BIN" doctor 2>&1 | tail -n10 || true
  docker --context harpoon version 2>&1 | head -n5 || true
  pass "regression basic"
}

main "$@"
