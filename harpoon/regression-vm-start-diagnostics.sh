#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
MAIN="$ROOT/harpoon/Sources/main.swift"
MANAGER="$ROOT/harpoon/Sources/VMManager.swift"

grep -q 'HARPOON_VZ_START_DIAGNOSTICS' "$MAIN"
grep -q 'HARPOON_VZ_START_ERROR' "$MAIN"
grep -q 'NSUnderlyingErrorKey' "$MAIN"
grep -q 'vmStartDiagnosticMaxDepth = 4' "$MAIN"
grep -q 'vmStartDiagnosticMaxEntries = 24' "$MAIN"
grep -q 'vmStartDiagnosticMaxText = 240' "$MAIN"
grep -q 'configurationSummary' "$MANAGER"
! grep -q 'start FAIL .*userInfo' "$MAIN"
! grep -q 'vm.start.*retry' "$MAIN"

echo "VM_START_DIAGNOSTICS_OFFLINE_PASS"
