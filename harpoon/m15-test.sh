#!/bin/sh
set -eu
# M15 baseline — host-visible post-workload reclamation with pinned runtime identity
# Fail-fast syntax self-check (top-level workflow): both harnesses must pass bash -n before any live run
if command -v bash >/dev/null 2>&1; then
  bash -n "$0" 2>&1 || { echo "[m15] SYNTAX_FAIL bash -n $0" >&2; exit 2; }
  # also validate companion harness when present
  if [ -f "harpoon/m15-balloon-test.sh" ]; then bash -n "harpoon/m15-balloon-test.sh" 2>&1 || { echo "[m15] SYNTAX_FAIL bash -n harpoon/m15-balloon-test.sh" >&2; exit 2; }; fi
  if command -v sh >/dev/null 2>&1; then sh -n "$0" 2>&1 || { echo "[m15] SYNTAX_FAIL sh -n $0" >&2; exit 2; }; fi
fi

RESULT_DIR="harpoon/results/m15"
BIN="harpoon/build/harpoon"
mkdir -p "$RESULT_DIR"

say() { echo "[m15] $*"; }
blocked() { echo "[m15] BLOCKED $*"; }
warn() { echo "[m15] WARN $*"; }

# Archive existing results as invalid if they exist and would be overwritten (preserve provenance)
if [ -f "$RESULT_DIR/host.csv" ] && [ "$(wc -l < "$RESULT_DIR/host.csv" 2>/dev/null | tr -d ' ')" != "1" ]; then
  ts=$(date -u +%Y%m%d-%H%M%S 2>/dev/null || date +%Y%m%d-%H%M%S)
  arch="harpoon/results/m15-preserved-$ts"
  mkdir -p "$arch" 2>/dev/null || true
  cp "$RESULT_DIR"/host.csv "$arch"/ 2>/dev/null || true
  cp "$RESULT_DIR"/tier-status.csv "$arch"/ 2>/dev/null || true
  cp "$RESULT_DIR"/guest.csv "$arch"/ 2>/dev/null || true
  say "preserved prior run to $arch"
fi

get_disk_path() {
  disk=$(harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('disk','') or d.get('diskPath',''))" 2>/dev/null || echo "")
  if [ -n "$disk" ] && [ -f "$disk" ]; then
    real=$(realpath "$disk" 2>/dev/null || readlink -f "$disk" 2>/dev/null || echo "$disk")
    echo "$real"; return
  fi
  for cand in "/tmp/harpoon-runtime/data/harpoon-root.img" "$HOME/Library/Application Support/Harpoon/data/harpoon-root.img" "spike2/cache/harpoon-root.img"; do
    if [ -f "$cand" ]; then
      real=$(realpath "$cand" 2>/dev/null || readlink -f "$cand" 2>/dev/null || echo "$cand")
      echo "$real"; return
    fi
  done
  echo "/tmp/harpoon-runtime/data/harpoon-root.img"
}

find_vm_pid() {
  disk="$1"
  disk_real=$(realpath "$disk" 2>/dev/null || readlink -f "$disk" 2>/dev/null || echo "$disk")
  disk_tmp=$(echo "$disk" | sed 's|^/private/tmp|/tmp|')
  disk_priv=$(echo "$disk" | sed 's|^/tmp|/private/tmp|')
  disk_real_tmp=$(echo "$disk_real" | sed 's|^/private/tmp|/tmp|')
  disk_real_priv=$(echo "$disk_real" | sed 's|^/tmp|/private/tmp|')
  pids=$(lsof -n 2>/dev/null | grep -F -e "$disk" -e "$disk_real" -e "$disk_tmp" -e "$disk_priv" -e "$disk_real_tmp" -e "$disk_real_priv" 2>/dev/null | awk '{print $2}' | sort -u | grep -E '^[0-9]+$' || echo "")
  count=$(echo "$pids" | grep -E '^[0-9]+$' | wc -l | tr -d ' ')
  if [ "$count" -eq 1 ]; then
    pid=$(echo "$pids" | head -n1 | tr -d ' ')
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then echo "$pid"; return; fi
  elif [ "$count" -gt 1 ]; then
    echo "VM_PID_AMBIGUOUS"; return
  fi
  pids2=$(lsof -n 2>/dev/null | grep -F "harpoon-root.img" 2>/dev/null | awk '{print $2}' | sort -u | grep -E '^[0-9]+$' || echo "")
  count2=$(echo "$pids2" | grep -E '^[0-9]+$' | wc -l | tr -d ' ')
  if [ "$count2" -eq 1 ]; then pid=$(echo "$pids2" | head -n1 | tr -d ' '); if kill -0 "$pid" 2>/dev/null; then echo "$pid"; return; fi; elif [ "$count2" -gt 1 ]; then echo "VM_PID_AMBIGUOUS"; return; fi
  echo ""
}

# Proven harpoon PID discovery: status --json + runtime.pid files (authoritative), NOT /tmp/harpoon.pid (legacy) nor bare pgrep
read_harpoon_pid() {
  pid=""
  # 1) status --json (authoritative when running)
  pid=$(harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('pid',''))" 2>/dev/null | tr -d ' \n' || echo "")
  if echo "$pid" | grep -qE '^[0-9]+$' && kill -0 "$pid" 2>/dev/null; then echo "$pid"; return; fi
  # 2) runtime.pid files (HarpoonPaths.pidFile)
  for cand in "/tmp/harpoon-runtime/runtime.pid" "$HOME/Library/Application Support/Harpoon/runtime.pid"; do
    if [ -f "$cand" ]; then
      p=$(tr -d ' \n' < "$cand" 2>/dev/null | grep -E '^[0-9]+$' || echo "")
      if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then echo "$p"; return; fi
    fi
  done
  # 3) no fallback to /tmp/harpoon.pid (legacy stale) — treat as unavailable
  echo ""
}

check_runtime() {
  pid=$(read_harpoon_pid)
  if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then return 1; fi
  if ! harpoon/build/harpoon status 2>&1 | grep -qi "running"; then return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then return 1; fi
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then return 1; fi
  return 0
}

# Wait for launcher->runtime transition to settle and pin stable identities
# Sets EXPECTED_HARPOON_PID and EXPECTED_VM_PID on success
wait_for_stable_runtime() {
  say "waiting for stable runtime identity..."
  stable_count=0
  last_pid=""
  tries=30
  n=0
  EXPECTED_HARPOON_PID=""
  EXPECTED_VM_PID=""
  while [ "$n" -lt "$tries" ]; do
    pid=$(read_harpoon_pid)
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && harpoon/build/harpoon status 2>&1 | grep -qi "running" && [ -S /tmp/harpoon-docker.sock ] && docker --context harpoon version 2>&1 | grep -q "Server"; then
      if [ "$pid" = "$last_pid" ] && [ -n "$pid" ]; then
        stable_count=$((stable_count+1))
      else
        stable_count=1
        last_pid="$pid"
      fi
      if [ "$stable_count" -ge 3 ]; then
        EXPECTED_HARPOON_PID="$pid"
        disk=$(get_disk_path)
        vm=$(find_vm_pid "$disk")
        if [ -n "$vm" ] && echo "$vm" | grep -qE '^[0-9]+$' && kill -0 "$vm" 2>/dev/null; then
          EXPECTED_VM_PID="$vm"
          say "pinned runtime harpoon_pid=$EXPECTED_HARPOON_PID vm_pid=$EXPECTED_VM_PID (stable $stable_count checks)"
          return 0
        fi
      fi
    else
      stable_count=0
      last_pid=""
    fi
    sleep 0.5
    n=$((n+1))
  done
  warn "stable runtime not achieved after $tries tries (last harpoon pid=$last_pid)"
  return 1
}

verify_pinned_runtime() {
  # emits RUNTIME_IDENTITY_CHANGED or RUNTIME_LOST and returns 1 on mismatch
  cur_pid=$(read_harpoon_pid)
  if [ -z "$cur_pid" ] || ! kill -0 "$cur_pid" 2>/dev/null; then echo "RUNTIME_LOST"; return 1; fi
  if [ -n "$EXPECTED_HARPOON_PID" ] && [ "$cur_pid" != "$EXPECTED_HARPOON_PID" ]; then echo "RUNTIME_IDENTITY_CHANGED harpoon $EXPECTED_HARPOON_PID -> $cur_pid"; return 1; fi
  disk=$(get_disk_path)
  cur_vm=$(find_vm_pid "$disk")
  if [ -z "$cur_vm" ] || ! echo "$cur_vm" | grep -qE '^[0-9]+$' || ! kill -0 "$cur_vm" 2>/dev/null; then echo "RUNTIME_LOST vm"; return 1; fi
  if [ -n "$EXPECTED_VM_PID" ] && [ "$cur_vm" != "$EXPECTED_VM_PID" ]; then echo "RUNTIME_IDENTITY_CHANGED vm $EXPECTED_VM_PID -> $cur_vm"; return 1; fi
  if ! harpoon/build/harpoon status 2>&1 | grep -qi "running"; then echo "RUNTIME_LOST status not running"; return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then echo "RUNTIME_LOST socket"; return 1; fi
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then echo "RUNTIME_LOST docker Server"; return 1; fi
  return 0
}

host_measure() {
  tier="$1"; phase="$2"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  # verify pinned identity before sample
  if [ -n "$EXPECTED_HARPOON_PID" ]; then
    if ! verify_pinned_runtime 2>&1; then
      reason=$(verify_pinned_runtime 2>&1 || true)
      echo "$ts,$tier,$phase,$EXPECTED_HARPOON_PID,HARPOON_RSS_UNAVAILABLE,,VM_PID_UNAVAILABLE,VM_RSS_UNAVAILABLE,,VM_PID_UNAVAILABLE," >> "$RESULT_DIR/host.csv"
      return 1
    fi
  else
    if ! check_runtime; then return 1; fi
  fi
  pid=$(read_harpoon_pid)
  harpoon_rss=""; harpoon_cpu=""
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    harpoon_rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    harpoon_cpu=$(ps -o %cpu= -p "$pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo "")
  fi
  disk=$(get_disk_path)
  vm_pid_raw=$(find_vm_pid "$disk")
  vm_pid=""; vm_rss=""; vm_cpu=""; combined=""; balloon_target=""
  if [ "$vm_pid_raw" = "VM_PID_AMBIGUOUS" ]; then
    echo "$ts,$tier,$phase,$pid,$harpoon_rss,$harpoon_cpu,VM_PID_AMBIGUOUS,VM_PID_AMBIGUOUS,VM_PID_AMBIGUOUS,VM_PID_AMBIGUOUS,$balloon_target" >> "$RESULT_DIR/host.csv"
    return 1
  fi
  vm_pid="$vm_pid_raw"
  # enforce pinned vm identity
  if [ -n "$EXPECTED_VM_PID" ] && [ -n "$vm_pid" ] && [ "$vm_pid" != "$EXPECTED_VM_PID" ]; then
    echo "$ts,$tier,$phase,$pid,$harpoon_rss,$harpoon_cpu,$vm_pid,VM_RSS_UNAVAILABLE,,RUNTIME_IDENTITY_CHANGED,$balloon_target" >> "$RESULT_DIR/host.csv"
    return 1
  fi
  if [ -n "$vm_pid" ] && kill -0 "$vm_pid" 2>/dev/null; then
    vm_rss=$(ps -o rss= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9]+$' || echo "")
    vm_cpu=$(ps -o %cpu= -p "$vm_pid" 2>/dev/null | tr -d ' ' | grep -E '^[0-9.]+$' || echo "")
  fi
  if ! echo "$harpoon_rss" | grep -qE '^[0-9]+$'; then harpoon_rss=""; fi
  if ! echo "$vm_rss" | grep -qE '^[0-9]+$'; then vm_rss=""; fi
  if [ -z "$pid" ] || [ -z "$harpoon_rss" ] || [ -z "$vm_pid" ] || [ -z "$vm_rss" ]; then
    if [ -z "$vm_pid" ]; then vm_pid="VM_PID_UNAVAILABLE"; fi
    if [ -z "$vm_rss" ]; then vm_rss="VM_RSS_UNAVAILABLE"; fi
    if [ -z "$harpoon_rss" ]; then harpoon_rss="HARPOON_RSS_UNAVAILABLE"; fi
    combined="VM_PID_UNAVAILABLE"
    log_path=$(harpoon/build/harpoon logs --path 2>/dev/null | head -n1 | tr -d '\r\n' || echo "")
    if [ -z "$log_path" ] || [ ! -f "$log_path" ]; then
      for cand in "$HOME/Library/Application Support/Harpoon/harpoon.log" /tmp/harpoon-runtime/harpoon.log; do if [ -f "$cand" ]; then log_path="$cand"; break; fi; done
    fi
    if [ -n "$log_path" ] && [ -f "$log_path" ]; then
      balloon_target=$(grep "HARPOON_BALLOON_TARGET_SET" "$log_path" 2>/dev/null | tail -n1 | awk '{print $NF}' || echo "")
    fi
    echo "$ts,$tier,$phase,$pid,$harpoon_rss,$harpoon_cpu,$vm_pid,$vm_rss,$vm_cpu,$combined,$balloon_target" >> "$RESULT_DIR/host.csv"
    return 1
  fi
  combined=$((harpoon_rss + vm_rss))
  balloon_target=""
  log_path=$(harpoon/build/harpoon logs --path 2>/dev/null | head -n1 | tr -d '\r\n' || echo "")
  if [ -z "$log_path" ] || [ ! -f "$log_path" ]; then
    for cand in "$HOME/Library/Application Support/Harpoon/harpoon.log" /tmp/harpoon-runtime/harpoon.log; do if [ -f "$cand" ]; then log_path="$cand"; break; fi; done
  fi
  if [ -n "$log_path" ] && [ -f "$log_path" ]; then
    balloon_target=$(grep "HARPOON_BALLOON_TARGET_SET" "$log_path" 2>/dev/null | tail -n1 | awk '{print $NF}' || echo "")
  fi
  # final verify after measure
  if [ -n "$EXPECTED_HARPOON_PID" ] && ! verify_pinned_runtime >/dev/null 2>&1; then return 1; fi
  echo "$ts,$tier,$phase,$pid,$harpoon_rss,$harpoon_cpu,$vm_pid,$vm_rss,$vm_cpu,$combined,$balloon_target" >> "$RESULT_DIR/host.csv"
  return 0
}

guest_measure() {
  tier="$1"; phase="$2"
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  if [ -n "$EXPECTED_HARPOON_PID" ] && ! verify_pinned_runtime >/dev/null 2>&1; then return 1; fi
  if ! check_runtime; then return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then return 1; fi
  out=$(docker --context harpoon run --rm alpine:3.22 sh -c 'cat /proc/meminfo | grep -E "MemTotal|MemAvailable"; echo "---"; cat /proc/loadavg' 2>&1 || echo "guest_failed")
  if echo "$out" | grep -q "guest_failed"; then return 1; fi
  memtotal=$(echo "$out" | grep MemTotal | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
  memavail=$(echo "$out" | grep MemAvailable | awk '{print $2}' | grep -E '^[0-9]+$' || echo "")
  if [ -z "$memtotal" ] && [ -z "$memavail" ]; then return 1; fi
  echo "$ts,$tier,$phase,$memtotal,$memavail" >> "$RESULT_DIR/guest.csv"
  return 0
}
# headers
echo "timestamp,tier,phase,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,vm_pid,vm_rss_kib,vm_cpu_pct,combined_rss_kib,balloon_target_bytes" > "$RESULT_DIR/host.csv"
echo "timestamp,tier,phase,mem_total_kib,mem_available_kib" > "$RESULT_DIR/guest.csv"
echo "tier,status,detail" > "$RESULT_DIR/tier-status.csv"
echo "tier,phase,harpoon_rss,vm_rss,combined,memtotal,memavail" > "$RESULT_DIR/memory-load.csv"

for tier in 512 768 1024; do
  say "=== Tier $tier ==="
  # clean stale stop file that would trigger immediate shutdown (seen 13:13:53 STOP_FILE)
  rm -f /tmp/harpoon-stop 2>/dev/null || true
  for cfg in "/tmp/harpoon-runtime/config.json" "$HOME/Library/Application Support/Harpoon/config.json"; do
    if mkdir -p "$(dirname "$cfg")" 2>/dev/null; then echo "{\"memory\":$tier,\"cpus\":2}" > "$cfg" 2>/dev/null || true; fi
  done
  harpoon/build/harpoon stop 2>&1 | tail -n3 || true
  sleep 2
  # wait for lock release and clean stale pid
  rm -f /tmp/harpoon-runtime/runtime.pid "$HOME/Library/Application Support/Harpoon/runtime.pid" 2>/dev/null || true
  set +e
  out=$(HARPOON_MEMORY_MIB="$tier" harpoon/build/harpoon start 2>&1)
  echo "$out" | tail -n15 > "$RESULT_DIR/start-$tier.log"
  if echo "$out" | grep -q "VZErrorDomain 1"; then
    say "VZ transient $tier, retry once after 5s"
    sleep 5
    out=$(HARPOON_MEMORY_MIB="$tier" harpoon/build/harpoon start 2>&1)
    echo "$out" | tail -n15 > "$RESULT_DIR/start-$tier.log"
    if echo "$out" | grep -q "VZErrorDomain 1"; then
      echo "$tier,HOST_VZ_START_FAILURE,VM failed to start" >> "$RESULT_DIR/tier-status.csv"
      blocked "tier $tier HOST_VZ_START_FAILURE"
      continue
    fi
  fi
  set -e
  sleep 2
  if ! check_runtime; then
    echo "$tier,RUNTIME_LOST_START,check_runtime failed" >> "$RESULT_DIR/tier-status.csv"
    blocked "tier $tier RUNTIME_LOST_START"
    continue
  fi
  for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then
    echo "$tier,DOCKER_NOT_READY,Server not ready" >> "$RESULT_DIR/tier-status.csv"
    blocked "tier $tier DOCKER_NOT_READY"
    continue
  fi
  # PIN stable runtime identity before any measurement (defect 2)
  EXPECTED_HARPOON_PID=""; EXPECTED_VM_PID=""
  if ! wait_for_stable_runtime; then
    echo "$tier,RUNTIME_IDENTITY_CHANGED,stable pin failed" >> "$RESULT_DIR/tier-status.csv"
    blocked "tier $tier RUNTIME_IDENTITY_CHANGED pin failed"
    continue
  fi
  say "idle settled 10s"
  sleep 10
  if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_IDLE,$reason" >> "$RESULT_DIR/tier-status.csv"; continue; fi
  host_measure "$tier" "idle-settled" || { reason=$(verify_pinned_runtime 2>&1 || echo "host_measure"); echo "$tier,RUNTIME_LOST_IDLE,$reason" >> "$RESULT_DIR/tier-status.csv"; continue; }
  guest_measure "$tier" "idle-settled" || warn "guest idle failed"
  # workload
  say "allocate 128M tier $tier"
  docker --context harpoon rm -f m15-stress 2>&1 | tail -n2 || true
  if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_WORKLOAD_CREATE,$reason" >> "$RESULT_DIR/tier-status.csv"; continue; fi
  set +e
  docker --context harpoon run -d --name m15-stress python:3-alpine sh -c 'python3 -c "
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
  if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_WORKLOAD_CREATE,$reason" >> "$RESULT_DIR/tier-status.csv"; docker --context harpoon rm -f m15-stress 2>&1 | tail -n2 || true; continue; fi
  running=$(docker --context harpoon inspect -f '{{.State.Running}}' m15-stress 2>&1 | head -n1 || echo "false")
  if [ "$running" != "true" ]; then
    echo "$tier,MEMORY_WORKLOAD_FAILED,inspect=$running" >> "$RESULT_DIR/tier-status.csv"
    docker --context harpoon logs m15-stress 2>&1 | tail -n10 || true
    docker --context harpoon rm -f m15-stress 2>&1 | tail -n2 || true
    warn "workload failed $tier"
    echo "$tier,LIVE" >> "$RESULT_DIR/tier-status.csv"
    harpoon/build/harpoon stop 2>&1 | tail -n3 || true
    continue
  fi
  # hold phases with pinned verify before each sample
  for phase in "hold-5s:5" "hold-10s:10" "hold-30s:30"; do
    name=$(echo "$phase" | cut -d: -f1); secs=$(echo "$phase" | cut -d: -f2)
    sleep "$secs"
    if ! docker --context harpoon inspect -f '{{.State.Running}}' m15-stress 2>&1 | grep -q "true"; then echo "$tier,RUNTIME_LOST_HOLD,$name not running" >> "$RESULT_DIR/tier-status.csv"; break; fi
    if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_HOLD,$reason $name" >> "$RESULT_DIR/tier-status.csv"; break; fi
    host_measure "$tier" "$name" || { reason=$(verify_pinned_runtime 2>&1 || echo "host $name"); echo "$tier,RUNTIME_LOST_HOLD,$reason" >> "$RESULT_DIR/tier-status.csv"; break; }
    guest_measure "$tier" "$name" || warn "guest $name failed"
  done
  # if already marked lost, stop tier
  if grep -q "^$tier,RUNTIME_LOST" "$RESULT_DIR/tier-status.csv" 2>/dev/null; then harpoon/build/harpoon stop 2>&1 | tail -n3 || true; sleep 2; continue; fi
  # release
  say "release workload tier $tier"
  docker --context harpoon rm -f m15-stress 2>&1 | tail -n2 || true
  sleep 2
  if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_RELEASE,$reason after rm" >> "$RESULT_DIR/tier-status.csv"; continue; fi
  host_measure "$tier" "release-immediate" || warn "host release-immediate failed"
  guest_measure "$tier" "release-immediate" || warn "guest release-immediate failed"
  for phase in "release-10s:10" "release-30s:30" "release-60s:60"; do
    name=$(echo "$phase" | cut -d: -f1); secs=$(echo "$phase" | cut -d: -f2)
    sleep "$secs"
    if ! verify_pinned_runtime 2>&1; then reason=$(verify_pinned_runtime 2>&1 || true); echo "$tier,RUNTIME_LOST_RELEASE,$reason $name" >> "$RESULT_DIR/tier-status.csv"; break; fi
    host_measure "$tier" "$name" || warn "host $name failed"
    guest_measure "$tier" "$name" || warn "guest $name failed"
  done
  # if any RUNTIME_LOST/IDENTITY changed, tier is INVALID, not PASS
  if grep -q "^$tier,RUNTIME_LOST\|^$tier,RUNTIME_IDENTITY" "$RESULT_DIR/tier-status.csv" 2>/dev/null; then
    blocked "tier $tier invalid due to runtime loss/identity"
    harpoon/build/harpoon stop 2>&1 | tail -n3 || true
    sleep 2
    continue
  fi
  echo "$tier,PASS,completed" >> "$RESULT_DIR/tier-status.csv"
  harpoon/build/harpoon stop 2>&1 | tail -n3 || true
  sleep 2
done
say "M15 baseline complete"
cat "$RESULT_DIR/tier-status.csv"
cat "$RESULT_DIR/host.csv"
cat "$RESULT_DIR/guest.csv"
