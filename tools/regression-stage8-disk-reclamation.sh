#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROBE="$ROOT/tools/stage-8-disk-reclamation-check.sh"
WORKFLOW="$ROOT/.github/workflows/stage-8-disk-reclamation.yml"

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
pass() { printf 'PASS %s\n' "$*"; }

bash -n "$PROBE" || fail "probe syntax"
grep -Fq 'fstrim /' "$PROBE" || fail "explicit root trim missing"
! grep -Fq 'fstrim -a' "$PROBE" || fail "broad fstrim form returned"
grep -Fq 'TRIM_RESULT="unsupported-or-rejected"' "$PROBE" || fail "unsupported trim classification missing"
grep -Fq 'discard-capability.txt' "$PROBE" || fail "discard capability artifact missing"
grep -Fq 'docker --context harpoon image rm "$IMAGE"' "$PROBE" || fail "test-image cleanup missing"
! grep -Eq 'docker --context harpoon (system|builder|image) prune' "$PROBE" || fail "broad Docker prune returned"
grep -Fq 'name: stage-8-disk-reclamation' "$WORKFLOW" || fail "artifact name missing"
grep -Fq 'if: always()' "$WORKFLOW" || fail "artifact upload is not failure-safe"

pass "Stage 8 probe safety and trim classification"
