#!/bin/sh
# Before a Git registration is removed on a journal, every build that can open that journal must be able to read the
# removal record (`gitdrop`) and name the journal schema it understands. This scans the given executables for both and
# fails for any that lacks them. It reads the files; it runs nothing.
#
#   tools/check-journal-readers.sh /Applications/Harpoon.app/Contents/MacOS/harpoon-desktop "$(command -v harpoon-deploy)"
set -eu

[ "$#" -gt 0 ] || { echo "usage: $0 BINARY..." >&2; exit 2; }
status=0
for bin in "$@"; do
  if [ ! -f "$bin" ]; then
    echo "MISSING  $bin"; status=1; continue
  fi
  have_record=0; have_schema=0
  strings -a "$bin" | grep -q 'gitdrop' && have_record=1
  strings -a "$bin" | grep -q 'written by a newer Harpoon' && have_schema=1
  if [ "$have_record" = 1 ] && [ "$have_schema" = 1 ]; then
    echo "READS    $bin"
  else
    echo "CANNOT   $bin  (gitdrop record: $have_record, schema check: $have_schema)"
    status=1
  fi
done
[ "$status" = 0 ] || echo "At least one build cannot read an upgraded journal: do not authorize a removal." >&2
exit "$status"
