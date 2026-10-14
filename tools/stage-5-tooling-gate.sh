#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
FAIL=0
BLOCKED=0
pass(){ printf 'PASS %s\n' "$*"; }
fail(){ printf 'FAIL %s\n' "$*"; FAIL=$((FAIL+1)); }

DOC=docs/underbelly/stage-5-tooling-audit.md
MAP=docs/underbelly/stage-5-tooling-inventory.json
[ -s "$DOC" ] && pass "tooling audit exists" || fail "tooling audit missing"
[ -s "$MAP" ] && pass "tooling inventory exists" || fail "tooling inventory missing"

if command -v python3 >/dev/null 2>&1; then
  python3 - "$MAP" <<'PY' || fail "tooling inventory validation"
import json, sys
d = json.load(open(sys.argv[1]))
assert d["schema"] == "harpoon.stage5.tooling-inventory.v1"
assert d["counts"]["extensionScripts"] >= 60
assert d["counts"]["extensionlessGuestRuntimeFiles"] == 3
assert d["classifications"]["COMPILED_CODE_CANDIDATE"]["sections"]
assert d["guestMigration"]["estimatedRuntimeSurfaceRemovedOrCollapsedLines"] == "770-1030"
print("PASS tooling inventory parses and covers mixed runtime sections")
PY
else
  fail "python3 missing"
fi

for marker in \
  KEEP_AS_SHELL COMPILED_CODE_CANDIDATE DELETE_OR_COLLAPSE_CANDIDATE \
  TEST_HARNESS_ONLY BUILD_RELEASE_ONLY SECURITY_SENSITIVE_REVIEW \
  HISTORICAL_OR_MILESTONE_ONLY harpoon-mgmt socat STAGE_5_GATE_PASS; do
  rg -q "$marker" "$DOC" && pass "audit marker $marker" || fail "audit marker $marker"
done

for source in \
  tools/guest-builder/src/init \
  tools/guest-builder/src/harpoon-mgmt \
  tools/guest-builder/src/harpoon-mgmt-wrapper \
  tools/guest-builder/build-initramfs.sh \
  tools/guest-builder/verify-guest.sh \
  tools/release/verify-runtime-closure.sh \
  tools/validate.sh \
  harpoon/regression-bridges.sh \
  harpoon/regression-foreground-status.sh; do
  [ -s "$source" ] && pass "audited surface present $source" || fail "audited surface missing $source"
done

while IFS= read -r file; do
  bash -n "$file" || fail "shell syntax $file"
done < <(find tools harpoon ui -type f -name '*.sh' -not -path '*/build/*' -not -path '*/cache/*' -print)
bash -n tools/guest-builder/src/init || fail "shell syntax tools/guest-builder/src/init"
bash -n tools/guest-builder/src/harpoon-mgmt-wrapper || fail "shell syntax tools/guest-builder/src/harpoon-mgmt-wrapper"

if command -v python3 >/dev/null 2>&1; then
  python3 - <<'PY' || fail "python syntax"
from pathlib import Path
for p in [Path("tools/guest-builder/src/harpoon-mgmt"), Path("tools/guest-builder/test-harpoon-mgmt.py")]:
    compile(p.read_text(), str(p), "exec")
print("PASS Python guest tooling syntax")
PY
fi

git diff --check >/dev/null 2>&1 && pass "diff whitespace" || fail "diff whitespace"

printf '\n## Prior barriers\n'
if [ "${HARPOON_STAGE5_SKIP_PRIOR_GATES:-0}" = 1 ]; then
  printf 'SKIP prior gates requested\n'
else
  HARPOON_GATE_LIVE="${HARPOON_GATE_LIVE:-auto}" \
    HARPOON_STAGE2_SAMPLES="${HARPOON_STAGE2_SAMPLES:-10}" \
    HARPOON_STAGE2_SAMPLE_DELAY="${HARPOON_STAGE2_SAMPLE_DELAY:-0.1}" \
    bash tools/stage-4-host-runtime-gate.sh >/tmp/harpoon-stage5-stage04.log 2>&1 \
    && pass "Stage 0-1, 2, 3, and 4 barriers" \
    || { rg -q 'STAGE_[234]_GATE_BLOCKED_ENVIRONMENT' /tmp/harpoon-stage5-stage04.log && BLOCKED=1 && echo "BLOCKED_ENVIRONMENT prior barriers" || fail "Stage 0-1, 2, 3, and 4 barriers (see /tmp/harpoon-stage5-stage04.log)"; }
fi

if [ "$BLOCKED" -eq 1 ]; then printf 'STAGE_5_GATE_BLOCKED_ENVIRONMENT\n'; exit 2; fi
if [ "$FAIL" -eq 0 ]; then
  printf 'STAGE_5_GATE_PASS\n'
  exit 0
fi
printf 'STAGE_5_GATE_FAIL failures=%s\n' "$FAIL"
exit 1
