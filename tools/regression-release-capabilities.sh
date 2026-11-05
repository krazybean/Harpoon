#!/bin/sh
# Exercises tools/check-release-bundle-capabilities.sh and tools/check-journal-readers.sh on synthetic bundles: it never
# builds, signs or runs anything. A good bundle passes; each way a bundle can go wrong is caught.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d /tmp/harpoon-bundle-capabilities.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

READER='gitdrop
this deployment journal was written by a newer Harpoon (journal schema 3); this build reads up to schema 2.'
make() { # make NAME : a good bundle
  d="$TMP/$1/Harpoon.app/Contents"; mkdir -p "$d/MacOS" "$d/Resources/harpoon/bin"
  printf '%s\n' "$READER" > "$d/MacOS/harpoon-desktop"
  printf 'Harpoon runtime\n' > "$d/Resources/harpoon/bin/harpoon"
  echo "$TMP/$1/Harpoon.app"
}
passes() { sh "$ROOT/tools/check-release-bundle-capabilities.sh" "$1" >/dev/null 2>&1; }
must_pass() { passes "$1" || { echo "FAIL: $2 should pass" >&2; exit 1; }; }
must_fail() { if passes "$1"; then echo "FAIL: $2 should be refused" >&2; exit 1; fi; }

good="$(make good)"; must_pass "$good" "a bundle with the reader and nothing acceptance-only"

app="$(make no-record)"; printf 'this deployment journal was written by a newer Harpoon; this build reads up to schema 2\n' > "$app/Contents/MacOS/harpoon-desktop"
must_fail "$app" "a desktop without the removal record reader"
app="$(make no-schema)"; printf 'gitdrop\n' > "$app/Contents/MacOS/harpoon-desktop"
must_fail "$app" "a desktop that does not refuse a newer schema by name"
app="$(make no-limit)"; printf 'gitdrop\nwritten by a newer Harpoon\n' > "$app/Contents/MacOS/harpoon-desktop"
must_fail "$app" "a desktop that does not name the schema it supports"
app="$(make banner)"; printf '*** ACCEPTANCE BUILD ***\n' >> "$app/Contents/MacOS/harpoon-desktop"
must_fail "$app" "a desktop that carries the acceptance banner"
app="$(make engine-reads)"; printf 'deployments.sqlite3\n' >> "$app/Contents/Resources/harpoon/bin/harpoon"
must_fail "$app" "a Swift runtime that opens the journal"
app="$(make acceptance-file)"; : > "$app/Contents/Resources/harpoon/bin/harpoon-acceptance-build"
must_fail "$app" "an acceptance executable"
app="$(make cli)"; : > "$app/Contents/Resources/harpoon/bin/harpoon-deploy"
must_fail "$app" "a deployment command-line tool"
app="$(make marker)"; : > "$app/Contents/Resources/git-removal.enabled"
must_fail "$app" "a removal enablement marker"
app="$(make nodesktop)"; rm "$app/Contents/MacOS/harpoon-desktop"
must_fail "$app" "a bundle without the desktop executable"
# the reader inventory names a binary that cannot read, and a binary that is not there
printf 'gitdrop\n' > "$TMP/old"
if sh "$ROOT/tools/check-journal-readers.sh" "$TMP/old" "$TMP/missing" >/dev/null 2>&1; then echo "FAIL: the reader inventory passed an old build" >&2; exit 1; fi
echo "release-capabilities PASS"
