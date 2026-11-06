#!/bin/sh
# Namespace isolation of the disk reports (#186): under HARPOON_TEST_TMPDIR, `doctor` and `disk status` resolve the root disk only
# inside the namespace, whether or not it has a disk. The namespace disk is 5 GiB, unlike any production disk, and every report is
# checked for any production path (the runtime ignores $HOME, so a leak names the real one). Read-only; no VM, no engine.
# Usage: sh harpoon/regression-namespace-disk-status.sh [harpoon-binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
unset HARPOON_DISK HARPOON_DISK_SIZE HARPOON_CPUS HARPOON_MEMORY_MIB HARPOON_ALLOW_TMP_FALLBACK HARPOON_TEST_MODE
export HARPOON_TEST_ROOT="$tmp/template.img"
head -c 1048576 /dev/zero > "$HARPOON_TEST_ROOT"
export HARPOON_TEST_TMPDIR="$tmp/ns"
mkdir -p "$HARPOON_TEST_TMPDIR"
iso="$HARPOON_TEST_TMPDIR/data/harpoon-root.img"
fail() { echo "namespace disk status FAIL: $*" >&2; exit 1; }
# both reports; their exit status is irrelevant to the paths they print
report() { { "$bin" disk status; "$bin" doctor; } 2>&1 || true; }

# 1. no isolated disk: reported as not created at the namespace path, nothing created, no production path
out=$(report)
case "$out" in *"Library/Application Support/Harpoon"*) fail "a production path appears with no isolated disk: $out";; esac
case "$out" in *"$iso (not yet provisioned)"*) ;; *) fail "disk status does not report $iso as not yet provisioned: $out";; esac
case "$out" in *"at $iso on first start"*) ;; *) fail "doctor does not report $iso as not yet provisioned: $out";; esac
[ ! -e "$HARPOON_TEST_TMPDIR/data" ] || fail "inspection created the namespace data directory"

# 2. an isolated disk: that one is reported and left untouched
mkdir -p "$(dirname "$iso")"
truncate -s 5G "$iso"
sig=$(stat -f '%z %m %i' "$iso")
out=$(report)
case "$out" in *"Library/Application Support/Harpoon"*) fail "a production path appears with an isolated disk: $out";; esac
case "$out" in *"Backing file:      $iso"*) ;; *) fail "disk status does not report $iso: $out";; esac
case "$out" in *"5 GiB"*) ;; *) fail "the isolated disk's size is not reported: $out";; esac
[ "$(stat -f '%z %m %i' "$iso")" = "$sig" ] || fail "inspection changed the isolated disk"
echo "namespace disk status OK"
