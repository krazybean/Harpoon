#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
FAIL=0
BLOCKED=0
fail() { echo "FAIL $*"; FAIL=$((FAIL + 1)); }
pass() { echo "PASS $*"; }
INIT=tools/guest-builder/src/init
MAP=docs/underbelly/stage-3-guest-boot-phases.json
DOC=docs/underbelly/stage-3-guest-boot-audit.md
[ -s "$INIT" ] && pass "guest init source exists" || fail "guest init source missing"
[ -s "$MAP" ] && pass "machine-readable phase map exists" || fail "phase map missing"
[ -s "$DOC" ] && pass "authoritative audit exists" || fail "audit missing"
if command -v python3 >/dev/null 2>&1; then
  python3 - "$MAP" "$INIT" "$DOC" <<'PY'
import json,sys
mapping,init,doc=sys.argv[1:]
data=json.load(open(mapping)); text=open(init).read(); audit=open(doc).read()
assert data["schema"] == "harpoon.stage3.guest-boot-phases.v1"
assert set(data["ownership"]) >= {"guest-init", "guest-runtime", "host-runtime", "builder"}
assert len(data["phases"]) >= 15
assert all(p.get("classification") and p.get("boundary") and p.get("lines") for p in data["phases"])
assert "switch_root" in text and "HARPOON_DOCKER_READY" in text and "HARPOON_MGMT_READY" in text
assert "BOOT_CRITICAL" in audit and "REDUNDANT_OR_HISTORICAL" in audit
print("PASS phase map parses and covers boot markers")
PY
  [ "$?" -eq 0 ] || fail "phase map/source consistency"
else
  fail "python3 missing for map validation"
fi
for marker in HARPOON_PIVOT HARPOON_RUNTIME_REFRESH_DONE HARPOON_MODULE_REFRESH_DONE HARPOON_DISK_CHECK_START HARPOON_NET_READY HARPOON_DOCKER_READY HARPOON_MGMT_READY; do
  rg -q "$marker" "$INIT" && pass "source marker $marker" || fail "source marker $marker"
done
printf '\n## Existing barriers (read-only mode)\n'
if [ "${HARPOON_STAGE3_SKIP_PRIOR_GATES:-0}" = 1 ]; then
  echo "SKIP prior gates requested"
else
  HARPOON_GATE_LIVE="${HARPOON_GATE_LIVE:-auto}" bash tools/stage-0-1-gate.sh >/tmp/harpoon-stage3-stage01.log 2>&1 && pass "Stage 0-1 gate" || fail "Stage 0-1 gate (see /tmp/harpoon-stage3-stage01.log)"
  HARPOON_STAGE2_SAMPLES="${HARPOON_STAGE2_SAMPLES:-10}" bash tools/stage-2-resource-gate.sh >/tmp/harpoon-stage3-stage02.log 2>&1 && pass "Stage 2 gate" || { rg -q STAGE_2_GATE_BLOCKED_ENVIRONMENT /tmp/harpoon-stage3-stage02.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT Stage 2 gate" || fail "Stage 2 gate (see /tmp/harpoon-stage3-stage02.log)"; }
fi
if [ "$BLOCKED" -eq 1 ]; then echo STAGE_3_GATE_BLOCKED_ENVIRONMENT; exit 2; fi
if [ "$FAIL" -eq 0 ]; then echo STAGE_3_GATE_PASS; exit 0; fi
echo "STAGE_3_GATE_FAIL failures=$FAIL"; exit 1
