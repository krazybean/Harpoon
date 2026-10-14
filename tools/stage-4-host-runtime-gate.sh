#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
FAIL=0
BLOCKED=0
pass(){ printf 'PASS %s\n' "$*"; }
fail(){ printf 'FAIL %s\n' "$*"; FAIL=$((FAIL+1)); }
DOC=docs/underbelly/stage-4-host-runtime-audit.md
MAP=docs/underbelly/stage-4-host-runtime-ownership.json
[ -s "$DOC" ] && pass "host audit exists" || fail "host audit missing"
[ -s "$MAP" ] && pass "machine-readable ownership map exists" || fail "ownership map missing"
if command -v python3 >/dev/null 2>&1; then
  python3 - "$MAP" <<'PY' || exit 1
import json,sys
d=json.load(open(sys.argv[1]))
assert d["schema"] == "harpoon.stage4.host-runtime-ownership.v1"
assert len(d["components"]) >= 5 and len(d["sockets"]) >= 4 and len(d["timers"]) >= 5
assert d["authority"]["daemonConclusion"]
print("PASS ownership map parses and covers components/sockets/timers")
PY
  [ "$?" -eq 0 ] || fail "ownership map validation"
else fail "python3 missing"; fi
for marker in Lifecycle VMManager BridgeSet PortForwardManager "STAGE_4_GATE_PASS" "persistent host daemon"; do
  rg -q "$marker" "$DOC" && pass "audit marker $marker" || fail "audit marker $marker"
done
for source in harpoon/Sources/Lifecycle.swift harpoon/Sources/VMManager.swift harpoon/Sources/Bridges.swift harpoon/Sources/UnixBridge.swift harpoon/Sources/MgmtBridge.swift harpoon/Sources/PortForwardManager.swift harpoon/Sources/main.swift; do
  [ -s "$source" ] && pass "audited source present $source" || fail "audited source missing $source"
done
printf '\n## Prior barriers\n'
if [ "${HARPOON_STAGE4_SKIP_PRIOR_GATES:-0}" = 1 ]; then
  printf 'SKIP prior gates requested\n'
else
  HARPOON_GATE_LIVE="${HARPOON_GATE_LIVE:-auto}" bash tools/stage-0-1-gate.sh >/tmp/harpoon-stage4-stage01.log 2>&1 && pass "Stage 0-1 gate" || fail "Stage 0-1 gate (see /tmp/harpoon-stage4-stage01.log)"
  HARPOON_STAGE2_SAMPLES="${HARPOON_STAGE2_SAMPLES:-10}" bash tools/stage-2-resource-gate.sh >/tmp/harpoon-stage4-stage02.log 2>&1 && pass "Stage 2 gate" || { rg -q STAGE_2_GATE_BLOCKED_ENVIRONMENT /tmp/harpoon-stage4-stage02.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT Stage 2 gate" || fail "Stage 2 gate (see /tmp/harpoon-stage4-stage02.log)"; }
  HARPOON_STAGE3_SKIP_PRIOR_GATES=1 bash tools/stage-3-guest-boot-gate.sh >/tmp/harpoon-stage4-stage03.log 2>&1 && pass "Stage 3 gate" || { rg -q 'STAGE_[23]_GATE_BLOCKED_ENVIRONMENT' /tmp/harpoon-stage4-stage03.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT Stage 3 gate" || fail "Stage 3 gate (see /tmp/harpoon-stage4-stage03.log)"; }
fi
if [ "$BLOCKED" -eq 1 ]; then printf 'STAGE_4_GATE_BLOCKED_ENVIRONMENT\n'; exit 2; fi
if [ "$FAIL" -eq 0 ]; then printf 'STAGE_4_GATE_PASS\n'; exit 0; fi
printf 'STAGE_4_GATE_FAIL failures=%s\n' "$FAIL"; exit 1
