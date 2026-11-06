#!/bin/sh
# Resolving the root disk must not create it (#183). Inspection commands and rejected starts leave no disk behind; an explicit
# HARPOON_DISK never provisions the default one; an existing disk is never rewritten; the default disk is created only when a VM
# start is about to happen (unit-tested, see regression-root-disk-test.swift). Private HARPOON_TEST_TMPDIR namespace, a fake
# template, no VM: every `start` has a missing kernel and stops in validation. Usage: sh harpoon/regression-root-disk.sh [harpoon-binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HARPOON_TEST_TMPDIR="$tmp/ns"
export HARPOON_TEST_ROOT="$tmp/template.img"
export HARPOON_KERNEL="$tmp/no-such-kernel"
unset HARPOON_DISK HARPOON_DISK_SIZE HARPOON_CPUS HARPOON_MEMORY_MIB
mkdir -p "$HARPOON_TEST_TMPDIR"
head -c 1048576 /dev/zero > "$HARPOON_TEST_ROOT"
disk="$HARPOON_TEST_TMPDIR/data/harpoon-root.img"
fail() { echo "root disk FAIL: $*" >&2; exit 1; }
absent() { [ ! -e "$disk" ] || fail "$1: a disk was created at $disk"; }
size() { stat -f %z "$1"; }
# size, modification time and the first 64 KiB: a rewrite, truncate or resize shows in one of them, and the 8 GiB file is not read through
sig() { printf "%s %s %s" "$(stat -f %z "$1")" "$(stat -f %m "$1")" "$(head -c 65536 "$1" | shasum | cut -c1-40)"; }

# inspection commands
[ "$("$bin" status --json | plutil -extract diskPath raw -)" = "$disk" ] || fail "status --json does not report the default path"
absent "status --json"
"$bin" doctor >/dev/null 2>&1 || true;        absent "doctor"
"$bin" config show >/dev/null 2>&1 || true;   absent "config show"
"$bin" disk status >/dev/null 2>&1 || true;   absent "disk status"
"$bin" diagnostics snapshot --json >/dev/null 2>&1 || true; absent "diagnostics"

# an explicit path is reported as is, and the default is not created
other="$tmp/elsewhere.img"
[ "$(HARPOON_DISK="$other" "$bin" status --json | plutil -extract diskPath raw -)" = "$other" ] || fail "HARPOON_DISK is not reported"
absent "status with HARPOON_DISK"; [ ! -e "$other" ] || fail "HARPOON_DISK path was created"

# starts that are rejected before the VM would start create nothing
set +e
out=$("$bin" start 2>&1); code=$?
set -e
[ "$code" = 2 ] || fail "start with a missing kernel: exit $code ($out)"
absent "a rejected start"
set +e; out=$("$bin" start --disk "$other" 2>&1); code=$?; set -e
[ "$code" = 2 ] || fail "start --disk with a missing kernel: exit $code ($out)"
absent "a rejected start --disk"; [ ! -e "$other" ] || fail "--disk path was created"

# --disk-size is the explicit first provision and still works; an existing disk is then left alone
set +e; out=$("$bin" start --disk-size 8G 2>&1); code=$?; set -e
[ "$code" = 2 ] || fail "start --disk-size with a missing kernel: exit $code ($out)"
[ "$(size "$disk")" = 8589934592 ] || fail "--disk-size 8G did not provision 8G"
sum=$(sig "$disk")
set +e; out=$("$bin" start --disk-size 16G 2>&1); code=$?; set -e
[ "$code" = 1 ] || fail "start --disk-size 16G on an existing disk: exit $code ($out)"
[ "$(sig "$disk")" = "$sum" ] || fail "an existing disk was changed"
"$bin" status --json >/dev/null; "$bin" doctor >/dev/null 2>&1 || true
[ "$(sig "$disk")" = "$sum" ] || fail "an inspection changed an existing disk"
[ "$("$bin" status --json | plutil -extract state raw -)" = stopped ] || fail "something started"

# the default disk is created only at the start boundary (unit test of the resolution API)
S="$ROOT/harpoon/Sources"
cp "$ROOT/harpoon/regression-root-disk-test.swift" "$tmp/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$S/RuntimeConfig.swift" "$S/Version.swift" "$S/ForwardTarget.swift" "$S/HarpoonRuntimeState.swift" "$tmp/main.swift" -framework Virtualization -o "$tmp/unit" 2>&1 | grep -E "error" && fail "unit test did not compile" || true
rm -rf "$HARPOON_TEST_TMPDIR"; mkdir -p "$HARPOON_TEST_TMPDIR"
"$tmp/unit"
echo "root disk regression PASS"
