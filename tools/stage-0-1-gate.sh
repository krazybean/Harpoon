#!/usr/bin/env bash
set -u

# Stage 0–1 exit barrier. This script observes the existing runtime; it does
# not change production behavior or weaken the Node 20 validation gate.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT="${HARPOON_GATE_REPORT:-/tmp/harpoon-stage-0-1-gate-${STAMP}.log}"
LIVE_MODE="${HARPOON_GATE_LIVE:-auto}"
FAILURES=0
LIVE_SKIPPED=0

mkdir -p "$(dirname "$REPORT")"
exec > >(tee "$REPORT") 2>&1

pass() { printf 'PASS %s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*"; FAILURES=$((FAILURES + 1)); }
skip() { printf 'SKIP %s\n' "$*"; LIVE_SKIPPED=1; }

printf '# Harpoon Stage 0–1 exit gate\n\n'
printf '%s\n' "UTC: $STAMP" "Repository: $ROOT" "Live mode: $LIVE_MODE"
printf '\n## Repository identity\n'
branch="$(git symbolic-ref --short -q HEAD || printf 'DETACHED')"
head="$(git rev-parse HEAD 2>/dev/null || printf 'UNKNOWN')"
status="$(git status --short --branch 2>&1)"
printf 'Branch: %s\nHEAD: %s\nWorking tree:\n%s\n' "$branch" "$head" "$status"
[ "$branch" != UNKNOWN ] && [ "$head" != UNKNOWN ] && pass "repository state recorded" || fail "repository state unavailable"

printf '\n## Ownership and documentation barriers\n'
if rg -q '/tmp/harpoon-docker\.sock' docs/architecture/runtime-anatomy.md &&
   rg -q '/tmp/harpoon-mgmt\.sock' docs/architecture/runtime-anatomy.md &&
   rg -q 'Docker Engine owns|Docker Engine/containerd/runc remain' docs/architecture/runtime-anatomy.md &&
   rg -q 'inferred' docs/architecture/runtime-anatomy.md; then
  pass "runtime anatomy records observed sockets, ownership, and inferred facts"
else
  fail "runtime anatomy is missing required observed/inferred ownership statements"
fi
if rg -q 'stale|Stale|qualified' docs/architecture/runtime-anatomy.md docs/roadmap.md; then
  pass "stale architecture/roadmap metadata is tracked as debt or qualified history"
else
  fail "stale metadata is not explicitly tracked"
fi

printf '\n## Existing repository validation (Node 20)\n'
node20=""
for candidate in \
  "${HARPOON_NODE20_BIN:-}" \
  "$HOME/.nvm/versions/node/v20.20.2/bin" \
  "$HOME/.nvm/versions/node/v20.20.0/bin" \
  "$HOME/.nvm/versions/node/v20.12.1/bin" \
  "$HOME/.nvm/versions/node/v20.11.1/bin" \
  "/opt/homebrew/opt/node@20/bin" \
  "/usr/local/opt/node@20/bin"; do
  if [ -x "${candidate:-}/node" ]; then node20="$candidate"; break; fi
done
if [ -z "$node20" ]; then
  fail "Node 20 toolchain unavailable; tools/validate.sh was not rerouted to Node 22"
else
  printf 'Node: %s\n' "$("$node20/node" --version)"
  printf 'npm: %s\n' "$(PATH="$node20:$PATH" npm --version 2>&1)"
  if PATH="$node20:$PATH" node --version | grep -Eq '^v20\.' && PATH="$node20:$PATH" sh tools/validate.sh; then
    pass "existing repository validation under Node 20"
  else
    fail "existing repository validation under Node 20"
  fi
fi

printf '\n## Live VM/Docker acceptance\n'
BIN="${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}"
can_live=1
if [ "$LIVE_MODE" = never ]; then can_live=0; skip "live acceptance disabled by HARPOON_GATE_LIVE=never"; fi
if [ "$LIVE_MODE" != never ] && [ "$(uname -s)" != Darwin ]; then can_live=0; skip "live acceptance requires macOS"; fi
if [ "$LIVE_MODE" != never ] && [ ! -x "$BIN" ]; then can_live=0; fail "live acceptance binary missing: $BIN"; fi
if [ "$LIVE_MODE" != never ] && ! command -v docker >/dev/null 2>&1; then can_live=0; fail "live acceptance requires docker"; fi

if [ "$can_live" -eq 1 ]; then
  "$BIN" stop >/tmp/harpoon-stage-0-1-stop.log 2>&1 || true
  if ! "$BIN" start; then
    fail "Harpoon VM starts"
  else
    status_json="$($BIN status --json 2>/dev/null || printf '{}')"
    printf 'Status JSON: %s\n' "$status_json"
    state="$(printf '%s' "$status_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state","unknown"))' 2>/dev/null || printf unknown)"
    ready="$(printf '%s' "$status_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("dockerReady",False))' 2>/dev/null || printf False)"
    [ "$state" = running ] && pass "Harpoon VM running" || fail "Harpoon VM running (state=$state)"
    [ "$ready" = True ] && pass "Docker reaches ready" || fail "Docker reaches ready (dockerReady=$ready)"

    [ -S /tmp/harpoon-docker.sock ] && pass "Docker socket exists at /tmp/harpoon-docker.sock" || fail "Docker socket path"
    [ -S /tmp/harpoon-mgmt.sock ] && pass "management socket exists at /tmp/harpoon-mgmt.sock" || fail "management socket path"
    docker_endpoint="$(docker context inspect -f '{{.Endpoints.docker.Host}}' harpoon 2>/dev/null || printf unknown)"
    printf 'Docker context endpoint: %s\n' "$docker_endpoint"
    [ "$docker_endpoint" = unix:///tmp/harpoon-docker.sock ] && pass "Docker context uses actual socket path" || fail "Docker context socket path"

    if docker --context harpoon version 2>&1 | grep -q 'Server'; then pass "Docker API responds"; else fail "Docker API responds"; fi
    if docker --context harpoon run --rm alpine:3.22 true; then pass "basic Docker run"; else fail "basic Docker run"; fi
    if "$BIN" exec -- true; then pass "management exec smoke"; else fail "management exec smoke"; fi
    if command -v expect >/dev/null 2>&1; then
      shell_output="$(expect -c 'log_user 0; set timeout 10; spawn harpoon/build/harpoon shell; send -- "printf STAGE01_SHELL_PASS\\r"; send -- "exit\\r"; expect { -re {STAGE01_SHELL_PASS} {puts STAGE01_SHELL_PASS; close; exit 0} timeout {close; exit 124} eof {exit 1} }' 2>&1)"
      shell_rc=$?
      printf '%s\n' "$shell_output"
      [ "$shell_rc" -eq 0 ] && pass "management shell smoke" || fail "management shell smoke (rc=$shell_rc)"
    else
      fail "management shell smoke requires host expect for a bounded PTY probe"
    fi

    printf '\n### Observable runtime facts\n'
    ps -p "$(printf '%s' "$status_json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("pid", ""))' 2>/dev/null || true)" -o pid=,ppid=,command= 2>/dev/null || true
    ls -l /tmp/harpoon-docker.sock /tmp/harpoon-mgmt.sock 2>/dev/null || true
    "$BIN" stop >/tmp/harpoon-stage-0-1-stop-final.log 2>&1 || fail "Harpoon stops after live smoke"
  fi
fi

printf '\n## Single exit criterion\n'
if [ "$FAILURES" -eq 0 ] && [ "$LIVE_SKIPPED" -eq 0 ]; then
  printf 'STAGE_0_1_GATE_PASS\n'
  exit 0
fi
printf 'STAGE_0_1_GATE_FAIL failures=%s live_skipped=%s\n' "$FAILURES" "$LIVE_SKIPPED"
exit 1
