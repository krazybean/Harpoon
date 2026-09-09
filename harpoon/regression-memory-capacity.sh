#!/bin/sh
set -eu
bin=${1:-harpoon/build/harpoon}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HARPOON_TEST_TMPDIR="$tmp"

for mib in 512 768 1024 2048 4096 6144 8192; do
  "$bin" config set memory "$mib" >/dev/null
  test "$("$bin" config get memory)" = "$mib"
done
"$bin" config set memory 4096 >/dev/null
before=$(cat "$tmp/config.json")
! "$bin" config set memory 511 >/dev/null 2>&1
test "$(cat "$tmp/config.json")" = "$before"
! "$bin" config set memory 999999999 >/dev/null 2>&1
test "$(cat "$tmp/config.json")" = "$before"
test "$(HARPOON_MEMORY_MIB=6144 "$bin" status --json | plutil -extract memoryMiB raw -)" = 6144
echo "memory capacity regression PASS"
