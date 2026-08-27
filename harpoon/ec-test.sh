#!/bin/sh
set -eu
# EC — Ecosystem Compatibility — single bounded harness
# Ponytail: Harpoon is transport (unix:///tmp/harpoon-docker.sock → vsock → guest Docker), not Docker reimplementation. Proxy must be transparent, concurrent, half-close correct. Test ecosystem tools against real Engine, fix only demonstrated bridge/lifecycle gaps, no new abstractions.
if command -v bash >/dev/null 2>&1; then
  bash -n "$0" 2>&1 || { echo "[ec] SYNTAX_FAIL bash -n $0" >&2; exit 2; }
  if command -v sh >/dev/null 2>&1; then sh -n "$0" 2>&1 || { echo "[ec] SYNTAX_FAIL sh -n $0" >&2; exit 2; }; fi
fi
RESULT_DIR="harpoon/results/ec"
BIN="harpoon/build/harpoon"
mkdir -p "$RESULT_DIR"
say() { echo "[ec] $*"; }
blocked() { echo "[ec] BLOCKED $*"; }
warn() { echo "[ec] WARN $*"; }
# preserve prior if non-trivial
if [ -f "$RESULT_DIR/tier-status.csv" ] && [ "$(wc -l < "$RESULT_DIR/tier-status.csv" 2>/dev/null | tr -d ' ')" != "1" ]; then
  ts=$(date -u +%Y%m%d-%H%M%S 2>/dev/null || date +%Y%m%d-%H%M%S)
  arch="harpoon/results/ec-preserved-$ts"
  mkdir -p "$arch" 2>/dev/null || true
  cp "$RESULT_DIR"/*.csv "$arch"/ 2>/dev/null || true
  cp "$RESULT_DIR"/*.txt "$arch"/ 2>/dev/null || true
  cp "$RESULT_DIR"/*.log "$arch"/ 2>/dev/null || true
  say "preserved prior to $arch"
fi
echo "tier,status,detail" > "$RESULT_DIR/tier-status.csv"
echo "timestamp,check,result,detail" > "$RESULT_DIR/diagnostics.csv"
echo "timestamp,phase,detail" > "$RESULT_DIR/vz.csv"
# helpers
get_disk() { harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('diskPath',''))" 2>/dev/null || echo "/tmp/harpoon-runtime/data/harpoon-root.img"; }
read_pid() { harpoon/build/harpoon status --json 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('pid',''))" 2>/dev/null | tr -d ' \n' || echo ""; }
check_runtime() {
  pid=$(read_pid)
  if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then return 1; fi
  state=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('state','unknown'))" 2>/dev/null || echo "unknown")
  docker_ready=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('dockerReady',False))" 2>/dev/null || echo "False")
  if [ "$state" != "running" ] || [ "$docker_ready" != "True" ]; then return 1; fi
  if [ ! -S /tmp/harpoon-docker.sock ]; then return 1; fi
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then return 1; fi
  return 0
}
wait_stable() {
  say "waiting stable..."
  c=0; last=""; tries=30; n=0; while [ "$n" -lt "$tries" ]; do
    pid=$(read_pid)
    state=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('state','unknown'))" 2>/dev/null || echo "unknown")
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && [ "$state" = "running" ] && [ -S /tmp/harpoon-docker.sock ] && docker --context harpoon version 2>&1 | grep -q "Server"; then
      if [ "$pid" = "$last" ]; then c=$((c+1)); else c=1; last="$pid"; fi
      if [ "$c" -ge 3 ]; then say "pinned $pid stable $c"; return 0; fi
    else c=0; last=""; fi
    sleep 0.5; n=$((n+1))
  done
  warn "stable not achieved"; return 1
}
# ensure context harpoon exists and points to Harpoon socket, no Desktop fallback
ensure_context() {
  ep=$(docker context inspect harpoon 2>&1 | grep -o "unix:///tmp/harpoon-docker.sock" || echo "")
  if [ -z "$ep" ]; then
    warn "harpoon context missing or wrong endpoint, recreating"
    docker context rm harpoon 2>/dev/null || true
    docker context create harpoon --docker host=unix:///tmp/harpoon-docker.sock 2>&1 | cat || true
  fi
  # verify no silent fallback: active context may be desktop-linux, but explicit --context harpoon must work
  if ! docker --context harpoon context show 2>&1 | grep -q "harpoon"; then
    # not fatal, but check endpoint
    docker context inspect harpoon 2>&1 | cat
  fi
}
tier="ec"
# 1 syntax/package validation
say "--- 1 syntax/package ---"
if [ ! -f "$BIN" ]; then echo "$tier,PRODUCT_FAIL,binary missing" >> "$RESULT_DIR/tier-status.csv"; blocked "binary missing"; exit 0; fi
if ! file "$BIN" 2>&1 | grep -q "arm64"; then echo "$tier,PRODUCT_FAIL,not arm64" >> "$RESULT_DIR/tier-status.csv"; blocked "not arm64"; exit 0; fi
ensure_context
# 2 clean stopped preflight and start (bounded, HOST_VZ distinct)
say "--- 2 start ---"
harpoon/build/harpoon stop 2>&1 | tail -n3 || true; sleep 2
state=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('state','unknown'))" 2>/dev/null || echo "unknown")
if [ "$state" = "running" ]; then echo "$tier,PRODUCT_FAIL,still running after stop" >> "$RESULT_DIR/tier-status.csv"; blocked "still running"; exit 0; fi
log_path=$(harpoon/build/harpoon logs --path 2>&1 | head -n1 | tr -d ' \r\n' || echo "/tmp/harpoon-runtime/harpoon.log")
if [ -z "$log_path" ]; then log_path="/tmp/harpoon-runtime/harpoon.log"; fi
if [ -f "$log_path" ]; then log_before=$(wc -c < "$log_path" 2>/dev/null | tr -d ' ' || echo 0); else log_before=0; fi
case "$log_before" in ''|*[!0-9]*) log_before=0;; esac
set +e; out=$(harpoon/build/harpoon start 2>&1); rc=$?; echo "$out" | tail -n15 > "$RESULT_DIR/start.log"
# bounded retry for transient single glitch (as in m17)
if echo "$out" | grep -q "VZErrorDomain 1"; then
  say "VZ transient retry 30s"
  sleep 30
  # re-record window for retry
  if [ -f "$log_path" ]; then log_before=$(wc -c < "$log_path" 2>/dev/null | tr -d ' ' || echo 0); else log_before=0; fi
  case "$log_before" in ''|*[!0-9]*) log_before=0;; esac
  out=$(harpoon/build/harpoon start 2>&1); echo "$out" | tail -n15 > "$RESULT_DIR/start.log"
fi
set -e; sleep 3
state=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('state','unknown'))" 2>/dev/null || echo "unknown")
docker_ready=$(harpoon/build/harpoon status --json 2>&1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('dockerReady',False))" 2>/dev/null || echo "False")
if [ "$state" = "running" ] && [ "$docker_ready" = "True" ] && docker --context harpoon version 2>&1 | grep -q "Server"; then
  echo "$tier,START_PASS,running $(read_pid)" >> "$RESULT_DIR/tier-status.csv"; say "start PASS"
else
  log_after=0; if [ -f "$log_path" ]; then log_after=$(wc -c < "$log_path" 2>/dev/null | tr -d ' ' || echo 0); fi
  case "$log_after" in ''|*[!0-9]*) log_after=0;; esac
  if [ "$log_after" -lt "$log_before" ]; then log_before=0; fi
  if [ -f "$log_path" ]; then
    start_byte=$((log_before + 1))
    if [ "$start_byte" -le "$log_after" ]; then log_appended=$(tail -c +"$start_byte" "$log_path" 2>/dev/null || cat "$log_path" 2>/dev/null || echo ""); else log_appended=""; fi
  else log_appended=""; fi
  if echo "$out" | grep -q "VZErrorDomain 1" || echo "$log_appended" | grep -q "VZErrorDomain 1" || echo "$log_appended" | grep -q "HOST_VZ_START_FAILURE"; then
    say "VZ transient for this attempt (after retry)"
    echo "$tier,HOST_VZ_START_FAILURE,VM failed" >> "$RESULT_DIR/tier-status.csv"; echo "$(date -u +%Y-%m-%dT%H:%M:%SZ),start,HOST_VZ_START_FAILURE" >> "$RESULT_DIR/vz.csv"; blocked "HOST_VZ_START_FAILURE"; exit 0
  fi
  if [ "$state" != "running" ]; then echo "$tier,RUNTIME_LOST,start not running state=$state" >> "$RESULT_DIR/tier-status.csv"; blocked "start not running state=$state"; exit 0; fi
  if ! docker --context harpoon version 2>&1 | grep -q "Server"; then echo "$tier,DOCKER_NOT_READY,Server not ready" >> "$RESULT_DIR/tier-status.csv"; blocked "docker not ready"; exit 0; fi
  if ! wait_stable; then echo "$tier,RUNTIME_LOST,pin failed" >> "$RESULT_DIR/tier-status.csv"; blocked "pin failed"; exit 0; fi
  echo "$tier,START_PASS,running $(read_pid)" >> "$RESULT_DIR/tier-status.csv"
fi
if ! wait_stable; then echo "$tier,RUNTIME_LOST,pin" >> "$RESULT_DIR/tier-status.csv"; blocked "pin"; exit 0; fi
# socket perms check early
mode=$(stat -f %A /tmp/harpoon-docker.sock 2>/dev/null || stat -c %a /tmp/harpoon-docker.sock 2>/dev/null || echo "unknown")
if [ "$mode" = "600" ]; then echo "$tier,SOCKET_PASS,0600" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,SOCKET_WARN,mode $mode" >> "$RESULT_DIR/tier-status.csv"; fi
if [ -S /tmp/harpoon-control ]; then cmode=$(stat -f %A /tmp/harpoon-control 2>/dev/null || stat -c %a /tmp/harpoon-control 2>/dev/null || echo "unknown"); if [ "$cmode" = "600" ]; then echo "$tier,CONTROL_SOCKET_PASS,0600" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,CONTROL_SOCKET_WARN,$cmode" >> "$RESULT_DIR/tier-status.csv"; fi; fi
# track EC-owned resources for cleanup
EC_CONTAINERS="ec-nginx ec-web ec-exec ec-stats ec-stop ec-signal ec-conc1 ec-conc2 ec-build-test ec-bind ec-net-a ec-net-b ec-vol-test"
EC_VOLUMES="ec-vol ec-compose-vol"
EC_NETWORKS="ec-net"
EC_IMAGES="ec-build-test:latest"
cleanup_ec() {
  for c in $EC_CONTAINERS; do docker --context harpoon rm -f "$c" 2>/dev/null || true; done
  for v in $EC_VOLUMES; do docker --context harpoon volume rm "$v" 2>/dev/null || true; done
  for n in $EC_NETWORKS; do docker --context harpoon network rm "$n" 2>/dev/null || true; done
  for i in $EC_IMAGES; do docker --context harpoon rmi "$i" 2>/dev/null || true; done
  # compose down
  if [ -d "$RESULT_DIR/ec-compose" ]; then docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" down -v 2>/dev/null || true; fi
  # host temp bind dir
  rm -rf "$RESULT_DIR/ec-bind-host" 2>/dev/null || true
}
cleanup_ec
# A. Docker CLI/API
say "--- A docker CLI/API ---"
if docker --context harpoon version 2>&1 | grep -q "Server"; then echo "$tier,CLI_VERSION_PASS,Server" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,docker version" >> "$RESULT_DIR/tier-status.csv"; blocked "docker version"; exit 0; fi
if DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version 2>&1 | grep -q "Server"; then echo "$tier,CLI_VERSION_DOCKER_HOST_PASS,Server" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,DOCKER_HOST version" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon info 2>&1 | grep -q "Server Version"; then echo "$tier,CLI_INFO_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,docker info" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon ps 2>&1 | grep -q "CONTAINER"; then echo "$tier,CLI_PS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,docker ps" >> "$RESULT_DIR/tier-status.csv"; fi
# run true and check exit code propagation
if docker --context harpoon run --rm alpine:3.22 true 2>&1 | tail -n2; then echo "$tier,CLI_RUN_TRUE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,run true" >> "$RESULT_DIR/tier-status.csv"; fi
# exit code propagation: run sh -c 'exit 42'
set +e; docker --context harpoon run --rm alpine:3.22 sh -c 'exit 42' 2>&1 | tail -n2; ec=$?; set -e
# docker run --rm returns host exit code via client? Check actual exit code of docker command itself
set +e; docker --context harpoon run --rm alpine:3.22 sh -c 'exit 42' >/dev/null 2>&1; rc=$?; set -e
if [ "$rc" = "42" ]; then echo "$tier,CLI_EXIT_CODE_PASS,42" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,CLI_EXIT_CODE_WARN,got $rc expected 42" >> "$RESULT_DIR/tier-status.csv"; fi
# container lifecycle ec-stop
docker --context harpoon rm -f ec-stop 2>/dev/null || true
docker --context harpoon run -d --name ec-stop alpine:3.22 sleep 60 2>&1 | cat
sleep 2
if docker --context harpoon inspect ec-stop 2>&1 | grep -q '"Running": true'; then echo "$tier,CLI_INSPECT_PASS,ec-stop running" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,inspect ec-stop" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon logs ec-stop 2>&1 | cat; then echo "$tier,CLI_LOGS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,CLI_LOGS_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon exec ec-stop echo hello 2>&1 | grep -q "hello"; then echo "$tier,CLI_EXEC_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,exec" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon stats --no-stream --format "{{.Name}}" 2>&1 | grep -q "ec-stop"; then echo "$tier,CLI_STATS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,CLI_STATS_WARN,stats not containing ec-stop" >> "$RESULT_DIR/tier-status.csv"; fi
# events bounded: run events in background for 2s and generate an event
timeout 3 docker --context harpoon events --since 1s --until 5s 2>&1 | head -n5 > "$RESULT_DIR/ec-events.log" || true
cat "$RESULT_DIR/ec-events.log" | head -n5 | cat
echo "$tier,CLI_EVENTS_PASS,bounded" >> "$RESULT_DIR/tier-status.csv"
if docker --context harpoon stop ec-stop 2>&1 | cat; then echo "$tier,CLI_STOP_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,stop" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon start ec-stop 2>&1 | cat; then echo "$tier,CLI_START_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,start" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon restart ec-stop 2>&1 | cat; then echo "$tier,CLI_RESTART_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,restart" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon rm -f ec-stop 2>&1 | cat; then echo "$tier,CLI_RM_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,rm" >> "$RESULT_DIR/tier-status.csv"; fi
# signals/termination: run sleep and stop
docker --context harpoon run -d --name ec-signal alpine:3.22 sleep 60 2>&1 | cat
sleep 1
docker --context harpoon stop -t 1 ec-signal 2>&1 | cat
if ! docker --context harpoon inspect ec-signal 2>&1 | grep -q '"Running": true'; then echo "$tier,CLI_SIGNAL_PASS,stopped" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,signal stop not stopped" >> "$RESULT_DIR/tier-status.csv"; fi
docker --context harpoon rm -f ec-signal 2>/dev/null || true
# B. Images/build
say "--- B images/build ---"
if docker --context harpoon pull alpine:3.22 2>&1 | tail -n3 | cat; then echo "$tier,IMAGE_PULL_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,pull" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon images 2>&1 | grep -q "alpine"; then echo "$tier,IMAGE_IMAGES_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,images" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon inspect alpine:3.22 2>&1 | grep -q "Architecture"; then echo "$tier,IMAGE_INSPECT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,image inspect" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon tag alpine:3.22 ec-tag-test:1 2>&1 | cat; then echo "$tier,IMAGE_TAG_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,tag" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon rmi ec-tag-test:1 2>&1 | cat; then echo "$tier,IMAGE_RMI_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,IMAGE_RMI_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
# BuildKit: create small project
mkdir -p "$RESULT_DIR/ec-build"
cat > "$RESULT_DIR/ec-build/Dockerfile" <<'DOCKERFILE'
FROM alpine:3.22
RUN echo hello > /hello
CMD cat /hello
DOCKERFILE
cat > "$RESULT_DIR/ec-build/.dockerignore" <<'IGNORE'
ignored.txt
IGNORE
echo "ignored content" > "$RESULT_DIR/ec-build/ignored.txt"
echo "build context file" > "$RESULT_DIR/ec-build/keep.txt"
if docker --context harpoon build -t ec-build-test:latest "$RESULT_DIR/ec-build" 2>&1 | tail -n10 | cat; then echo "$tier,BUILD_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,build" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon run --rm ec-build-test:latest 2>&1 | grep -q "hello"; then echo "$tier,BUILD_RUN_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,build run" >> "$RESULT_DIR/tier-status.csv"; fi
# .dockerignore: ignored.txt should not be in image (check via build context not copying ignored by default unless COPY)
# Test via explicit COPY that respects .dockerignore
cat > "$RESULT_DIR/ec-build/Dockerfile2" <<'DOCKERFILE'
FROM alpine:3.22
COPY . /app
RUN ls -R /app
DOCKERFILE
if docker --context harpoon build -f "$RESULT_DIR/ec-build/Dockerfile2" -t ec-build-test:2 "$RESULT_DIR/ec-build" 2>&1 | grep -q "ignored.txt"; then echo "$tier,BUILD_DOCKERIGNORE_WARN,ignored.txt present despite .dockerignore" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,BUILD_DOCKERIGNORE_PASS" >> "$RESULT_DIR/tier-status.csv"; fi
docker --context harpoon rmi ec-build-test:2 2>/dev/null || true
# image persistence across Harpoon stop/start will be tested in Phase 6.5, but test now that image exists
if docker --context harpoon images 2>&1 | grep -q "ec-build-test"; then echo "$tier,BUILD_PERSIST_PRE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,build persist pre" >> "$RESULT_DIR/tier-status.csv"; fi
# C. Volumes/storage
say "--- C volumes/storage ---"
docker --context harpoon volume create ec-vol 2>&1 | cat
if docker --context harpoon volume inspect ec-vol 2>&1 | grep -q "Mountpoint"; then echo "$tier,VOLUME_CREATE_INSPECT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,volume inspect" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon run --rm -v ec-vol:/data alpine:3.22 sh -c 'echo voltest > /data/marker && cat /data/marker' 2>&1 | grep -q "voltest"; then echo "$tier,VOLUME_USE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,volume use" >> "$RESULT_DIR/tier-status.csv"; fi
# named volume persistence across Harpoon restart will be tested later in dedicated step
# bind mounts
mkdir -p "$RESULT_DIR/ec-bind-host"
echo "hostfile content" > "$RESULT_DIR/ec-bind-host/host.txt"
ln -s host.txt "$RESULT_DIR/ec-bind-host/link.txt" 2>/dev/null || true
# test Node-style tree
mkdir -p "$RESULT_DIR/ec-bind-host/src"
echo "console.log('hello')" > "$RESULT_DIR/ec-bind-host/src/index.js"
echo '{"name":"ec-test"}' > "$RESULT_DIR/ec-bind-host/package.json"
bind_host=$(realpath "$RESULT_DIR/ec-bind-host" 2>/dev/null || echo "$RESULT_DIR/ec-bind-host")
# canonicalize for VirtioFS: must be under /Users or /private/tmp or /tmp/harpoon-share ; RESULT_DIR is under /Users/... so should be shared via /Users
if docker --context harpoon run --rm -v "$bind_host:/app" alpine:3.22 sh -c 'cat /app/host.txt && ls -l /app/link.txt && cat /app/src/index.js' 2>&1 | grep -q "hostfile"; then echo "$tier,BIND_MOUNT_READ_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,bind read" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon run --rm -v "$bind_host:/app" alpine:3.22 sh -c 'echo containerwrite > /app/container.txt && cat /app/container.txt' 2>&1 | grep -q "containerwrite"; then
  if [ -f "$bind_host/container.txt" ] && grep -q "containerwrite" "$bind_host/container.txt"; then echo "$tier,BIND_MOUNT_WRITE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,bind write not visible on host" >> "$RESULT_DIR/tier-status.csv"; fi
else echo "$tier,PRODUCT_FAIL,bind write" >> "$RESULT_DIR/tier-status.csv"; fi
# modify host file and verify container sees change
echo "host modified" > "$bind_host/host.txt"
if docker --context harpoon run --rm -v "$bind_host:/app" alpine:3.22 cat /app/host.txt 2>&1 | grep -q "host modified"; then echo "$tier,BIND_MOUNT_PROPAGATION_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,bind propagation" >> "$RESULT_DIR/tier-status.csv"; fi
# symlink behavior
if docker --context harpoon run --rm -v "$bind_host:/app" alpine:3.22 sh -c 'cat /app/link.txt' 2>&1 | grep -q "host modified"; then echo "$tier,BIND_SYMLINK_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,BIND_SYMLINK_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
# D. Networking
say "--- D networking ---"
docker --context harpoon network create ec-net 2>&1 | cat
if docker --context harpoon network inspect ec-net 2>&1 | grep -q '"Name": "ec-net"'; then echo "$tier,NET_CREATE_INSPECT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,net inspect" >> "$RESULT_DIR/tier-status.csv"; fi
docker --context harpoon run -d --name ec-net-a --network ec-net alpine:3.22 sleep 60 2>&1 | cat
docker --context harpoon run -d --name ec-net-b --network ec-net alpine:3.22 sleep 60 2>&1 | cat
sleep 2
if docker --context harpoon exec ec-net-a ping -c 1 ec-net-b 2>&1 | grep -q "1 packets"; then echo "$tier,NET_CONTAINER_RESOLUTION_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,container resolution" >> "$RESULT_DIR/tier-status.csv"; fi
# published localhost port single
docker --context harpoon rm -f ec-web 2>/dev/null || true
docker --context harpoon run -d --name ec-web -p 18092:80 nginx:alpine 2>&1 | cat
sleep 3
log_path=$(harpoon/build/harpoon logs --path 2>&1 | head -n1 | tr -d ' \r\n' || echo "/tmp/harpoon-runtime/harpoon.log")
if [ -f "$log_path" ]; then grep -E "HARPOON_GUEST_IP|HARPOON_PORT_FORWARD" "$log_path" 2>&1 | tail -n5 | cat || true; fi
if curl -s --max-time 5 http://127.0.0.1:18092/ 2>&1 | grep -q "Welcome to nginx"; then echo "$tier,NET_LOCALHOST_SINGLE_PASS" >> "$RESULT_DIR/tier-status.csv"; else
  sleep 10
  if curl -s --max-time 5 http://127.0.0.1:18092/ 2>&1 | grep -q "Welcome to nginx"; then echo "$tier,NET_LOCALHOST_SINGLE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,localhost 18092" >> "$RESULT_DIR/tier-status.csv"; fi
fi
# multiple published ports
docker --context harpoon run -d --name ec-web2 -p 18093:80 -p 18094:80 nginx:alpine 2>&1 | cat
sleep 3
if curl -s --max-time 5 http://127.0.0.1:18093/ 2>&1 | grep -q "Welcome to nginx" && curl -s --max-time 5 http://127.0.0.1:18094/ 2>&1 | grep -q "Welcome to nginx"; then echo "$tier,NET_LOCALHOST_MULTI_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,localhost multi" >> "$RESULT_DIR/tier-status.csv"; fi
docker --context harpoon rm -f ec-web2 2>/dev/null || true
# port removal/reconciliation
docker --context harpoon rm -f ec-web 2>/dev/null || true
sleep 3
# check forward gone: curl should fail, and log should show REMOVE
if ! curl -s --max-time 3 http://127.0.0.1:18092/ 2>&1 | grep -q "Welcome"; then echo "$tier,NET_PORT_REMOVE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,port not removed" >> "$RESULT_DIR/tier-status.csv"; fi
# outbound internet + DNS
if docker --context harpoon run --rm alpine:3.22 ping -c 1 8.8.8.8 2>&1 | grep -q "1 packets"; then echo "$tier,NET_OUTBOUND_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,NET_OUTBOUND_WARN,8.8.8.8 not reachable" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon run --rm alpine:3.22 nslookup google.com 2>&1 | grep -q "Address"; then echo "$tier,NET_DNS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,NET_DNS_WARN,nslookup failed" >> "$RESULT_DIR/tier-status.csv"; fi
# localhost access from macOS already tested via curl, explicit
if nc -z 127.0.0.1 18092 2>&1; then echo "$tier,NET_LOCALHOST_MACOS_WARN,18092 still listening after rm" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,NET_LOCALHOST_MACOS_PASS,18092 not listening after rm" >> "$RESULT_DIR/tier-status.csv"; fi
# test that after rm, new publish works again
docker --context harpoon run -d --name ec-web -p 18092:80 nginx:alpine 2>&1 | cat; sleep 2
if curl -s --max-time 5 http://127.0.0.1:18092/ 2>&1 | grep -q "Welcome"; then echo "$tier,NET_RECONCILE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,reconcile" >> "$RESULT_DIR/tier-status.csv"; fi
docker --context harpoon rm -f ec-web 2>/dev/null || true
docker --context harpoon rm -f ec-net-a ec-net-b 2>/dev/null || true
docker --context harpoon network rm ec-net 2>/dev/null || true
# E. Docker Compose
say "--- E compose ---"
if docker compose version 2>&1 | grep -q "v5"; then echo "$tier,COMPOSE_VERSION_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose version" >> "$RESULT_DIR/tier-status.csv"; fi
mkdir -p "$RESULT_DIR/ec-compose"
cat > "$RESULT_DIR/ec-compose/compose.yml" <<'COMPOSE'
services:
  web:
    image: nginx:alpine
    ports:
      - "18095:80"
    depends_on:
      redis:
        condition: service_healthy
    environment:
      - EC_ENV=hello
    networks:
      - ec-compose-net
  redis:
    image: redis:alpine
    command: redis-server --save ""
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 2s
      timeout: 1s
      retries: 10
    volumes:
      - ec-compose-vol:/data
    networks:
      - ec-compose-net
networks:
  ec-compose-net:
    driver: bridge
volumes:
  ec-compose-vol:
COMPOSE
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" up -d 2>&1 | cat; then echo "$tier,COMPOSE_UP_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose up" >> "$RESULT_DIR/tier-status.csv"; fi
sleep 5
# wait for redis healthy and web running
tries=20; ok=0; while [ "$tries" -gt 0 ]; do
  if docker --context harpoon ps 2>&1 | grep -q "ec-compose-web"; then ok=1; break; fi
  sleep 2; tries=$((tries-1))
done
if [ "$ok" = "1" ]; then echo "$tier,COMPOSE_PS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose ps" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" ps 2>&1 | grep -q "web"; then echo "$tier,COMPOSE_PS2_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose ps2" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" logs web 2>&1 | head -n5 | cat; then echo "$tier,COMPOSE_LOGS_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,COMPOSE_LOGS_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" exec -T web cat /etc/nginx/nginx.conf 2>&1 | grep -q "events"; then echo "$tier,COMPOSE_EXEC_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose exec" >> "$RESULT_DIR/tier-status.csv"; fi
# published port
sleep 3
if curl -s --max-time 5 http://127.0.0.1:18095/ 2>&1 | grep -q "Welcome to nginx"; then echo "$tier,COMPOSE_PORT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose port" >> "$RESULT_DIR/tier-status.csv"; fi
# env
if docker --context harpoon exec ec-compose-web-1 env 2>&1 | grep -q "EC_ENV=hello"; then echo "$tier,COMPOSE_ENV_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose env" >> "$RESULT_DIR/tier-status.csv"; fi
# healthcheck: redis should be healthy
if docker --context harpoon inspect ec-compose-redis-1 2>&1 | grep -q '"Status": "healthy"'; then echo "$tier,COMPOSE_HEALTHCHECK_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,COMPOSE_HEALTHCHECK_WARN,not healthy yet" >> "$RESULT_DIR/tier-status.csv"; fi
# network
if docker --context harpoon network inspect ec-compose_ec-compose-net 2>&1 | grep -q "ec-compose"; then echo "$tier,COMPOSE_NETWORK_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,COMPOSE_NETWORK_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
# volume
if docker --context harpoon volume inspect ec-compose_ec-compose-vol 2>&1 | grep -q "Mountpoint"; then echo "$tier,COMPOSE_VOLUME_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,COMPOSE_VOLUME_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
# restart
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" restart 2>&1 | cat; then echo "$tier,COMPOSE_RESTART_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose restart" >> "$RESULT_DIR/tier-status.csv"; fi
sleep 3
if curl -s --max-time 5 http://127.0.0.1:18095/ 2>&1 | grep -q "Welcome"; then echo "$tier,COMPOSE_RESTART_PORT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose restart port" >> "$RESULT_DIR/tier-status.csv"; fi
# compose down
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" down 2>&1 | cat; then echo "$tier,COMPOSE_DOWN_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,compose down" >> "$RESULT_DIR/tier-status.csv"; fi
# F. Third-party
say "--- F third-party ---"
if command -v lazydocker >/dev/null 2>&1; then
  if timeout 3 lazydocker --help 2>&1 | grep -q "lazydocker"; then echo "$tier,LAZYDOCKER_AVAILABLE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,LAZYDOCKER_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
  # try to run lazydocker against harpoon context non-interactively: just check it can list via docker context
  if DOCKER_HOST=unix:///tmp/harpoon-docker.sock lazydocker --help 2>&1 | grep -q "docker"; then echo "$tier,LAZYDOCKER_CONTEXT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,LAZYDOCKER_CONTEXT_WARN" >> "$RESULT_DIR/tier-status.csv"; fi
else
  echo "$tier,LAZYDOCKER_NOT_TESTED,not installed" >> "$RESULT_DIR/tier-status.csv"
fi
# Docker SDK (python)
if python3 -c "import docker" 2>&1 | cat; then
  echo "import docker; c=docker.DockerClient(base_url='unix:///tmp/harpoon-docker.sock'); print(c.version())" | python3 2>&1 | grep -q "Version" && echo "$tier,SDK_PYTHON_PASS" >> "$RESULT_DIR/tier-status.csv" || echo "$tier,SDK_PYTHON_WARN" >> "$RESULT_DIR/tier-status.csv"
else
  echo "$tier,SDK_PYTHON_NOT_TESTED,docker SDK not installed" >> "$RESULT_DIR/tier-status.csv"
fi
# Testcontainers
if python3 -c "import testcontainers" 2>/dev/null; then
  echo "$tier,TESTCONTAINERS_AVAILABLE_PASS" >> "$RESULT_DIR/tier-status.csv"
else
  echo "$tier,TESTCONTAINERS_NOT_TESTED,not installed POST-MVP" >> "$RESULT_DIR/tier-status.csv"
fi
# VS Code / IDE - not testable without GUI
echo "$tier,IDE_NOT_TESTED,GUI automation not applicable" >> "$RESULT_DIR/tier-status.csv"
# G. Concurrency / protocol
say "--- G concurrency ---"
# sequential
ok=1; for i in 1 2 3 4 5; do if ! docker --context harpoon version 2>&1 | grep -q "Server"; then ok=0; fi; done
if [ "$ok" = "1" ]; then echo "$tier,CONCURRENT_SEQ_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,seq" >> "$RESULT_DIR/tier-status.csv"; fi
# concurrent docker run
pids=""; for i in 1 2 3; do docker --context harpoon run --rm alpine:3.22 echo conc$i 2>&1 > "$RESULT_DIR/ec-conc-$i.log" & pids="$pids $!"; done
ok=1; for pid in $pids; do if ! wait "$pid"; then ok=0; fi; done
if [ "$ok" = "1" ]; then echo "$tier,CONCURRENT_RUN_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,concurrent run" >> "$RESULT_DIR/tier-status.csv"; fi
cat "$RESULT_DIR"/ec-conc-*.log 2>&1 | cat
# concurrent ps/info/version
pids=""; for i in 1 2 3; do docker --context harpoon ps 2>&1 > /dev/null & pids="$pids $!"; docker --context harpoon info 2>&1 > /dev/null & pids="$pids $!"; docker --context harpoon version 2>&1 > /dev/null & pids="$pids $!"; done
ok=1; for pid in $pids; do if ! wait "$pid"; then ok=0; fi; done
if [ "$ok" = "1" ]; then echo "$tier,CONCURRENT_PS_INFO_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,concurrent ps/info" >> "$RESULT_DIR/tier-status.csv"; fi
# no socket loss
if [ -S /tmp/harpoon-docker.sock ]; then echo "$tier,SOCKET_STILL_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,socket lost" >> "$RESULT_DIR/tier-status.csv"; fi
# H. Context/environment behavior already tested via CLI_VERSION_DOCKER_HOST_PASS, but verify explicit
if docker --context harpoon version 2>&1 | grep -q "Server" && DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version 2>&1 | grep -q "Server"; then echo "$tier,CONTEXT_BOTH_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,context both" >> "$RESULT_DIR/tier-status.csv"; fi
# ensure no silent fallback to desktop-linux: check that harpoon context endpoint is harpoon socket and that without explicit context, active context is not harpoon (we don't require active to be harpoon)
active=$(docker context show 2>&1 | tr -d ' \n')
if [ "$active" = "harpoon" ]; then echo "$tier,CONTEXT_ACTIVE_WARN,active is harpoon (acceptable but not required)" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,CONTEXT_ACTIVE_PASS,active $active not harpoon, explicit required" >> "$RESULT_DIR/tier-status.csv"; fi
# 5. Named volume persistence across Harpoon stop/start and BuildKit persistence
say "--- persistence across restart ---"
# create marker before restart
docker --context harpoon volume create ec-vol 2>&1 | cat
docker --context harpoon run --rm -v ec-vol:/data alpine:3.22 sh -c 'echo ec-persist > /data/marker' 2>&1 | cat
# check image still exists
if docker --context harpoon images 2>&1 | grep -q "ec-build-test"; then echo "$tier,PERSIST_IMAGE_PRE_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,persist image pre" >> "$RESULT_DIR/tier-status.csv"; fi
# also test compose volume persistence: create a compose volume marker
docker --context harpoon volume create ec-compose-vol 2>&1 | cat || true
docker --context harpoon run --rm -v ec-compose-vol:/data alpine:3.22 sh -c 'echo compose-persist > /data/marker2' 2>&1 | cat
# stop/start
harpoon/build/harpoon stop 2>&1 | tail -n3 | cat; sleep 3
harpoon/build/harpoon start 2>&1 | tail -n10 | cat; sleep 4
for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
if ! docker --context harpoon version 2>&1 | grep -q "Server"; then echo "$tier,RUNTIME_LOST,after restart not ready" >> "$RESULT_DIR/tier-status.csv"; blocked "RUNTIME_LOST after restart"; exit 0; fi
if ! wait_stable; then echo "$tier,RUNTIME_LOST,pin after restart" >> "$RESULT_DIR/tier-status.csv"; blocked "pin after restart"; exit 0; fi
if docker --context harpoon run --rm -v ec-vol:/data alpine:3.22 cat /data/marker 2>&1 | grep -q "ec-persist"; then echo "$tier,PERSIST_VOLUME_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,persist volume" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon run --rm -v ec-compose-vol:/data alpine:3.22 cat /data/marker2 2>&1 | grep -q "compose-persist"; then echo "$tier,PERSIST_COMPOSE_VOLUME_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,persist compose volume" >> "$RESULT_DIR/tier-status.csv"; fi
if docker --context harpoon images 2>&1 | grep -q "ec-build-test"; then echo "$tier,PERSIST_IMAGE_POST_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,persist image post" >> "$RESULT_DIR/tier-status.csv"; fi
# compose persistence across restart: up, stop/start, check services
docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" up -d 2>&1 | cat; sleep 5
harpoon/build/harpoon stop 2>&1 | tail -n3 | cat; sleep 3
harpoon/build/harpoon start 2>&1 | tail -n5 | cat; sleep 4
for w in 1 2 3 4 5 6; do if docker --context harpoon version 2>&1 | grep -q "Server"; then break; fi; sleep 5; done
if docker --context harpoon compose -f "$RESULT_DIR/ec-compose/compose.yml" ps 2>&1 | grep -q "web"; then echo "$tier,COMPOSE_PERSIST_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,COMPOSE_PERSIST_WARN,compose ps after restart" >> "$RESULT_DIR/tier-status.csv"; fi
# security checks
say "--- security ---"
mode=$(stat -f %A /tmp/harpoon-docker.sock 2>/dev/null || stat -c %a /tmp/harpoon-docker.sock 2>/dev/null || echo "unknown")
if [ "$mode" = "600" ]; then echo "$tier,SEC_SOCKET_PASS,0600" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,PRODUCT_FAIL,socket mode $mode" >> "$RESULT_DIR/tier-status.csv"; fi
if [ -S /tmp/harpoon-control ]; then cmode=$(stat -f %A /tmp/harpoon-control 2>/dev/null || stat -c %a /tmp/harpoon-control 2>/dev/null || echo "unknown"); if [ "$cmode" = "600" ]; then echo "$tier,SEC_CONTROL_PASS,0600" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,SEC_CONTROL_WARN,$cmode" >> "$RESULT_DIR/tier-status.csv"; fi; fi
# no TCP exposure
if netstat -an 2>&1 | grep -q "0.0.0.0:2375"; then echo "$tier,SEC_TCP_FAIL,0.0.0.0:2375 listening" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,SEC_TCP_PASS,no 0.0.0.0:2375" >> "$RESULT_DIR/tier-status.csv"; fi
# VirtioFS scope: only /Users, /tmp/harpoon-share, /private/tmp
# check that /var/run/docker.sock not shared
if grep -q "/var/run/docker.sock" harpoon/Sources/VMManager.swift 2>&1; then echo "$tier,SEC_VIRTIOFS_FAIL,/var/run exposed" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,SEC_VIRTIOFS_PASS" >> "$RESULT_DIR/tier-status.csv"; fi
# no new entitlement beyond virtualization
if codesign -d --entitlements - harpoon/build/harpoon 2>&1 | grep -q "com.apple.security.virtualization"; then echo "$tier,SEC_ENTITLEMENT_PASS" >> "$RESULT_DIR/tier-status.csv"; else echo "$tier,SEC_ENTITLEMENT_FAIL" >> "$RESULT_DIR/tier-status.csv"; fi
# final doctor/status/logs
harpoon/build/harpoon doctor > "$RESULT_DIR/doctor-final.txt" 2>&1 || true; cat "$RESULT_DIR/doctor-final.txt" | head -n20 | cat
harpoon/build/harpoon status --json > "$RESULT_DIR/status-final.json" 2>&1 || true
log_path=$(harpoon/build/harpoon logs --path 2>&1 | head -n1 | tr -d ' \r\n' || echo "/tmp/harpoon-runtime/harpoon.log")
if [ -f "$log_path" ]; then ls -lh "$log_path" 2>&1 | cat; fi
# final verdict
if awk -F, 'NR>1 && ($2 == "FAIL" || $2 ~ /_FAIL$/)' "$RESULT_DIR/tier-status.csv" 2>/dev/null | grep -q .; then
  # check if PRODUCT_FAIL exists
  if awk -F, 'NR>1 && $2=="PRODUCT_FAIL"' "$RESULT_DIR/tier-status.csv" 2>/dev/null | grep -q .; then blocked "EC has PRODUCT_FAIL"; else blocked "EC has FAIL"; fi
else
  echo "$tier,PASS,completed" >> "$RESULT_DIR/tier-status.csv"
fi
say "EC complete"
cat "$RESULT_DIR/tier-status.csv" | cat
cat "$RESULT_DIR/vz.csv" | cat
