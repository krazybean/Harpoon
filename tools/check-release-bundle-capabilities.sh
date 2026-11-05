#!/bin/sh
# The capability boundaries a Harpoon.app must have before it is released, checked on the built bundle (it reads the
# files; it runs nothing). verify-bundle.sh runs it for every bundle the build, DMG and release scripts verify.
#
#   1. The one executable that opens the deployment journal can read a journal a removal has upgraded and names the
#      schema it supports (tools/check-journal-readers.sh): Contents/MacOS/harpoon-desktop, which has the Rust journal
#      crates linked in. A bundle that cannot would strand every user who later removes something.
#   2. The Swift runtime (Resources/harpoon/bin/harpoon) does not open the journal. If it ever starts to, it becomes a
#      reader that this script (and the reader inventory) must know about, so it fails until a person decides.
#   3. Nothing that only an acceptance run may use is in the bundle: no acceptance executable, no `harpoon-deploy`
#      command-line tool, no removal enablement marker, and the desktop does not carry the acceptance build's banner.
#      (The capability itself is closed in the shipping gate; a packaging test in deploy-cli pins that in source.)
set -eu

APP="${1:?usage: $0 Harpoon.app}"
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
DESKTOP="$APP/Contents/MacOS/harpoon-desktop"
ENGINE="$APP/Contents/Resources/harpoon/bin/harpoon"
status=0
fail() { echo "[bundle-capabilities] FAIL: $*" >&2; status=1; }
pass() { echo "[bundle-capabilities] PASS: $*" >&2; }

if [ -f "$DESKTOP" ]; then
  if sh "$ROOT/tools/check-journal-readers.sh" "$DESKTOP" >/dev/null 2>&1; then
    pass "harpoon-desktop reads schema-2 journals and refuses a newer schema by name"
  else
    fail "harpoon-desktop cannot read an upgraded journal (tools/check-journal-readers.sh)"
  fi
  if strings -a "$DESKTOP" | grep -q 'ACCEPTANCE BUILD'; then
    fail "harpoon-desktop carries the acceptance build's banner"
  else
    pass "harpoon-desktop is not an acceptance build"
  fi
else
  fail "$DESKTOP is missing"
fi

if [ -f "$ENGINE" ]; then
  if strings -a "$ENGINE" | grep -q 'deployments\.sqlite3\|gitdrop'; then
    fail "the Swift runtime now references the deployment journal: it is a reader and must be inventoried"
  else
    pass "the Swift runtime does not open the deployment journal"
  fi
else
  fail "$ENGINE is missing"
fi

found="$(find "$APP" \( -iname '*acceptance*' -o -name 'harpoon-deploy' -o -name 'harpoon-deploy-*' -o -name 'git-removal.enabled' \) 2>/dev/null | head -n 5)"
if [ -n "$found" ]; then
  fail "acceptance-only or command-line deployment files are in the bundle:
$found"
else
  pass "no acceptance executable, deployment CLI or removal marker in the bundle"
fi

exit "$status"
