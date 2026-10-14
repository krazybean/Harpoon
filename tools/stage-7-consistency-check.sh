#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ARTIFACT_DIR="${HARPOON_CONSISTENCY_ARTIFACT_DIR:-/tmp/harpoon-stage7-consistency}"
mkdir -p "$ARTIFACT_DIR"
started_by_check=0

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
cleanup() {
  if [ "$started_by_check" -eq 1 ]; then
    harpoon/build/harpoon stop >"$ARTIFACT_DIR/final-stop.log" 2>&1 || true
  fi
}
trap cleanup EXIT

if ! docker --context harpoon version >/dev/null 2>&1; then
  harpoon/build/harpoon start >"$ARTIFACT_DIR/start.log" 2>&1 || fail "Harpoon start"
  started_by_check=1
fi

docker --context harpoon ps -a >"$ARTIFACT_DIR/baseline.txt" 2>&1 || fail "Docker baseline"
baseline_rows="$(docker --context harpoon ps -aq 2>/dev/null || true)"
[ -z "$baseline_rows" ] || fail "baseline is not empty; preserved existing containers"

set +e
HARPOON_GATE_LIVE=auto bash tools/stage-7-diagnostics-gate.sh >"$ARTIFACT_DIR/stage-7.log" 2>&1
gate_rc=$?
set -e
[ "$gate_rc" -eq 0 ] || fail "Stage 7 exit=$gate_rc"

if ! docker --context harpoon version >/dev/null 2>&1; then
  harpoon/build/harpoon start >"$ARTIFACT_DIR/restart.log" 2>&1 || fail "Harpoon restart verification"
fi
docker --context harpoon ps -a >"$ARTIFACT_DIR/post-restart.txt" 2>&1 || fail "post-restart Docker check"
post_restart_rows="$(docker --context harpoon ps -aq 2>/dev/null || true)"
[ -z "$post_restart_rows" ] || fail "post-restart state is not empty"

if [ "$started_by_check" -eq 1 ]; then
  harpoon/build/harpoon stop >"$ARTIFACT_DIR/final-stop.log" 2>&1 || fail "final Harpoon stop"
  started_by_check=0
fi

printf 'STAGE_7_CONSISTENCY_PASS\n'
