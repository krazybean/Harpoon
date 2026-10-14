#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; cd "$ROOT"; FAIL=0; BLOCKED=0
pass(){ printf 'PASS %s\n' "$*"; }; fail(){ printf 'FAIL %s\n' "$*"; FAIL=$((FAIL+1)); }
DOC=docs/underbelly/stage-7-diagnostics-contract.md; FIX=docs/underbelly/stage-7-diagnostics-snapshot.json
[ -s "$DOC" ] && pass "diagnostics contract exists" || fail "diagnostics contract missing"
python3 - "$FIX" <<'PY' || fail "diagnostics fixture validation"
import json,sys
d=json.load(open(sys.argv[1])); assert d["schema"] == "harpoon.diagnostics.snapshot.v1"
assert set(d["domains"]) == {"HOST","GUEST","WORKLOADS"}; assert "grandTotal" in d["semantics"]
print("PASS diagnostics fixture parses with separate domains")
PY
for marker in HOST GUEST WORKLOADS provenance cgroup unavailable grandTotal diagnosticsSnapshot mgmtExecCapture; do
  rg -q "$marker" "$DOC" harpoon/Sources/HarpoonDiagnosticsCLI.swift || fail "marker $marker"
done
for source in harpoon/Sources/HarpoonDiagnosticsCLI.swift harpoon/Sources/HarpoonMgmtCLI.swift harpoon/Sources/main.swift; do [ -s "$source" ] && pass "source present $source" || fail "source missing $source"; done
bash -n tools/stage-7-diagnostics-gate.sh harpoon/build.sh || fail "shell syntax"
git diff --check >/dev/null 2>&1 && pass "diff whitespace" || fail "diff whitespace"
if [ "${HARPOON_STAGE7_SKIP_PRIOR_GATES:-0}" = 1 ]; then pass "prior gates skipped by request"; else HARPOON_GATE_LIVE="${HARPOON_GATE_LIVE:-auto}" HARPOON_STAGE2_SAMPLES="${HARPOON_STAGE2_SAMPLES:-10}" bash tools/stage-6-docker-boundary-gate.sh >/tmp/harpoon-stage7-stage6.log 2>&1 && pass "Stage 0-6 barriers" || { rg -q 'STAGE_[23456]_GATE_BLOCKED_ENVIRONMENT' /tmp/harpoon-stage7-stage6.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT prior barriers" || fail "Stage 0-6 barriers (see /tmp/harpoon-stage7-stage6.log)"; }; fi
if [ "${HARPOON_STAGE7_BUILD:-0}" = 1 ]; then [ -x harpoon/build/harpoon ] || fail "built Harpoon binary missing"; fi
if [ "$BLOCKED" -eq 1 ]; then printf 'STAGE_7_GATE_BLOCKED_ENVIRONMENT\n'; exit 2; fi
if [ "$FAIL" -eq 0 ]; then printf 'STAGE_7_GATE_PASS\n'; exit 0; fi
printf 'STAGE_7_GATE_FAIL failures=%s\n' "$FAIL"; exit 1
