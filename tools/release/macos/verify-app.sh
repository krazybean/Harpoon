#!/bin/bash
set -euo pipefail
# ponytail: structural verification of Harpoon.app (pre-signing, ad-hoc is ok)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
APP="${1:-$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app}"
if [ ! -d "$APP" ]; then echo "[verify-app] FAIL: Harpoon.app not found at $APP" >&2; exit 1; fi
echo "[verify-app] structural verify $APP" >&2
bash "$REPO_ROOT/tools/verify-bundle.sh" "$APP" 2>&1 | tee /tmp/harpoon-verify-app.log
status=${PIPESTATUS[0]}
# verify-bundle returns 0 for pre-release ad-hoc (INFO outer signing pending), 1 for real FAIL
if [ $status -ne 0 ]; then echo "[verify-app] FAIL structural" >&2; exit $status; fi
# Additional pre-signing checks: nested valid, no spike, no dev path (already in verify-bundle)
echo "[verify-app] PASS structural (pre-signing, ad-hoc ok)" >&2
