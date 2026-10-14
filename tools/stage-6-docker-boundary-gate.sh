#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
FAIL=0
BLOCKED=0
pass(){ printf 'PASS %s\n' "$*"; }
fail(){ printf 'FAIL %s\n' "$*"; FAIL=$((FAIL+1)); }
DOC=docs/underbelly/stage-6-docker-boundary-audit.md
MAP=docs/underbelly/stage-6-docker-boundary-matrix.json
[ -s "$DOC" ] && pass "Stage 6 audit exists" || fail "Stage 6 audit missing"
[ -s "$MAP" ] && pass "Stage 6 boundary matrix exists" || fail "Stage 6 boundary matrix missing"
python3 - "$MAP" <<'PY' || fail "boundary matrix validation"
import json,sys
d=json.load(open(sys.argv[1]))
assert d["schema"] == "harpoon.stage6.docker-boundary-matrix.v1"
assert d["status"] == "STAGE_6_GATE_PASS"
assert len(d["concerns"]) >= 10
for c in d["concerns"]:
    assert c["authority"] and c["interaction"] and "competing_state" in c and c["classification"] and c["sources"]
assert "must not infer or persist" in d["invariant"]
print("PASS boundary matrix parses and covers authority/mode/state/classification/source")
PY
for marker in Docker containerd runc PortForwardManager REQUIRED_BOUNDARY_TRANSLATION SAFE_OBSERVATION THIN_CLIENT_CONTROL DUPLICATED_SEMANTICS_REVIEW STAGE_6_GATE_PASS; do
  rg -q "$marker" "$DOC" && pass "audit marker $marker" || fail "audit marker $marker"
done
for source in harpoon/Sources/UnixBridge.swift harpoon/Sources/PortForwardManager.swift harpoon/Sources/HostPathTranslator.swift tools/guest-builder/src/init ui/harpoon-desktop/src-tauri/src/main.rs; do
  [ -s "$source" ] && pass "audited source present $source" || fail "audited source missing $source"
done
bash -n tools/stage-6-docker-boundary-gate.sh || fail "gate syntax"
git diff --check >/dev/null 2>&1 && pass "diff whitespace" || fail "diff whitespace"
if [ "${HARPOON_STAGE6_SKIP_PRIOR_GATES:-0}" = 1 ]; then
  pass "prior gates skipped by request"
else
  HARPOON_GATE_LIVE="${HARPOON_GATE_LIVE:-auto}" HARPOON_STAGE2_SAMPLES="${HARPOON_STAGE2_SAMPLES:-10}" HARPOON_STAGE2_SAMPLE_DELAY="${HARPOON_STAGE2_SAMPLE_DELAY:-0.1}" bash tools/stage-5-tooling-gate.sh >/tmp/harpoon-stage6-stage5.log 2>&1 && pass "Stage 0-1 through Stage 5 barriers" || { rg -q 'STAGE_[2345]_GATE_BLOCKED_ENVIRONMENT' /tmp/harpoon-stage6-stage5.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT prior barriers" || fail "Stage 0-1 through Stage 5 barriers (see /tmp/harpoon-stage6-stage5.log)"; }
fi
if [ "${HARPOON_STAGE6_LIVE:-1}" = 1 ]; then
  started_for_probe=0
  probe_created=0
  probe_cleanup_failed=0
  name=""
  cleanup_probe() {
    [ "$probe_created" -eq 1 ] || return 0
    if ! docker --context harpoon rm -f "$name" >/dev/null 2>&1; then
      echo "FAIL Docker cleanup: unable to remove $name" >&2
      probe_cleanup_failed=1
      return 1
    fi
    if docker --context harpoon inspect "$name" >/dev/null 2>&1; then
      echo "FAIL Docker cleanup: $name still exists after removal" >&2
      probe_cleanup_failed=1
      return 1
    fi
    pass "Docker cleanup and absence verification"
    probe_created=0
    return 0
  }
  cleanup_on_exit() {
    cleanup_probe || probe_cleanup_failed=1
    if [ "$started_for_probe" -eq 1 ]; then
      if ! "$HARPOON_STAGE6_BIN" stop >/dev/null 2>&1; then
        echo "FAIL Harpoon stop after probe cleanup" >&2
      fi
      started_for_probe=0
    fi
  }
  trap cleanup_on_exit EXIT
  HARPOON_STAGE6_BIN="${HARPOON_STAGE6_BIN:-harpoon/build/harpoon}"
  if ! docker --context harpoon version >/dev/null 2>&1 && [ -x "$HARPOON_STAGE6_BIN" ]; then
    "$HARPOON_STAGE6_BIN" start >/tmp/harpoon-stage6-start.log 2>&1 && started_for_probe=1 || true
  fi
  if docker --context harpoon version >/dev/null 2>&1; then
    name="harpoon-stage6-boundary-$$"
    id="$(docker --context harpoon create --label com.krazybean.harpoon.validation=stage6 --label com.krazybean.harpoon.stage=6 --name "$name" alpine:3.22 sh -c 'echo stage6-log; sleep 60' 2>/dev/null || true)"
    [ -n "$id" ] && probe_created=1
    [ -n "$id" ] && pass "Docker create" || fail "Docker create"
    docker --context harpoon start "$name" >/dev/null 2>&1 && pass "Docker start" || fail "Docker start"
    docker --context harpoon inspect "$name" >/dev/null 2>&1 && pass "Docker inspect" || fail "Docker inspect"
    docker --context harpoon logs "$name" 2>/dev/null | rg -q stage6-log && pass "Docker logs" || fail "Docker logs"
    docker --context harpoon exec "$name" true >/dev/null 2>&1 && pass "Docker exec" || fail "Docker exec"
    docker --context harpoon stop "$name" >/dev/null 2>&1 && pass "Docker stop" || fail "Docker stop"
    cleanup_probe || fail "Docker cleanup"
  else
    fail "Docker context harpoon unavailable for live probe"
  fi
  if [ "$started_for_probe" -eq 1 ]; then
    "$HARPOON_STAGE6_BIN" stop >/dev/null 2>&1 && pass "Harpoon stop after probe cleanup" || fail "Harpoon stop after probe cleanup"
    started_for_probe=0
  fi
  [ "$probe_cleanup_failed" -eq 0 ] || fail "Docker cleanup was not verified"
else
  pass "live probe skipped by request"
fi
if [ "$BLOCKED" -eq 1 ]; then printf 'STAGE_6_GATE_BLOCKED_ENVIRONMENT\n'; exit 2; fi
if [ "$FAIL" -eq 0 ]; then printf 'STAGE_6_GATE_PASS\n'; exit 0; fi
printf 'STAGE_6_GATE_FAIL failures=%s\n' "$FAIL"; exit 1
