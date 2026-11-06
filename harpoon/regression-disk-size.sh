#!/bin/sh
# Size of a new root disk (#184): --disk-size > HARPOON_DISK_SIZE > config disk-size > 32 GiB, applied only when a disk is created.
# CLI layer and existing-disk preservation through the binary (every `start` has a missing kernel and stops in validation, so no VM
# starts); the environment/config layers and invalid values through a unit test of the resolution API. Private HARPOON_TEST_TMPDIR
# namespaces and a fake template. Usage: sh harpoon/regression-disk-size.sh [harpoon-binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HARPOON_TEST_ROOT="$tmp/template.img"
export HARPOON_KERNEL="$tmp/no-such-kernel"
unset HARPOON_DISK HARPOON_DISK_SIZE HARPOON_CPUS HARPOON_MEMORY_MIB
head -c 1048576 /dev/zero > "$HARPOON_TEST_ROOT"
fail() { echo "disk size FAIL: $*" >&2; exit 1; }
# a fresh namespace, optionally with a config.json
ns() { n=$((n + 1)); export HARPOON_TEST_TMPDIR="$tmp/ns$n"; mkdir -p "$HARPOON_TEST_TMPDIR"; disk="$HARPOON_TEST_TMPDIR/data/harpoon-root.img"; [ -z "${1:-}" ] || printf '%s\n' "$1" > "$HARPOON_TEST_TMPDIR/config.json"; }
n=0
sig() { printf "%s %s %s" "$(stat -f %z "$1")" "$(stat -f %m "$1")" "$(head -c 65536 "$1" | shasum | cut -c1-40)"; }
start() { set +e; out=$("$bin" start "$@" 2>&1); code=$?; set -e; }
G=1073741824

# --disk-size beats the environment and the config file
ns '{"diskSize":"16G"}'
HARPOON_DISK_SIZE=8G start --disk-size 4G
[ "$code" = 2 ] || fail "start --disk-size: exit $code ($out)"
[ "$(stat -f %z "$disk")" = $((4 * G)) ] || fail "--disk-size did not win over the environment and the config file"
# ... and the = form
ns '{"diskSize":"16G"}'
HARPOON_DISK_SIZE=8G start --disk-size=6G
[ "$(stat -f %z "$disk")" = $((6 * G)) ] || fail "--disk-size=6G did not win"

# invalid --disk-size is refused before anything is created, with the same messages as before
ns
start --disk-size 1G;     [ "$code" = 2 ] || fail "--disk-size 1G: exit $code"; case "$out" in *"at least 2G"*) ;; *) fail "--disk-size 1G: $out";; esac; [ ! -e "$disk" ] || fail "--disk-size 1G created a disk"
start --disk-size banana; [ "$code" = 2 ] || fail "--disk-size banana: exit $code"; case "$out" in *"invalid --disk-size"*) ;; *) fail "--disk-size banana: $out";; esac; [ ! -e "$disk" ] || fail "--disk-size banana created a disk"

# an existing disk is never resized or rewritten: not by --disk-size, the environment or the config file, not by inspection
ns '{"diskSize":"16G"}'
start --disk-size 4G; before=$(sig "$disk")
HARPOON_DISK_SIZE=64G start --disk-size 16G
[ "$code" = 1 ] || fail "--disk-size on an existing disk: exit $code ($out)"
case "$out" in *"use harpoon disk resize"*) ;; *) fail "--disk-size on an existing disk: $out";; esac
HARPOON_DISK_SIZE=64G start; [ "$code" = 2 ] || fail "start on an existing disk: exit $code"
[ "$(sig "$disk")" = "$before" ] || fail "an existing disk was changed by start"
for cmd in "status --json" "doctor" "config show" "diagnostics snapshot --json"; do
  HARPOON_DISK_SIZE=64G "$bin" $cmd >/dev/null 2>&1 || true
  [ "$(sig "$disk")" = "$before" ] || fail "'$cmd' changed an existing disk"
done

# inspection with every layer set creates nothing
ns '{"diskSize":"16G"}'
for cmd in "status --json" "doctor" "config show" "diagnostics snapshot --json"; do
  HARPOON_DISK_SIZE=8G "$bin" $cmd >/dev/null 2>&1 || true
  [ ! -e "$disk" ] || fail "'$cmd' created a disk"
done
# (`disk status` is not run here: with no disk in the namespace it falls back to the production disk, #186; the size a new disk gets is
# unit-tested below through the same function it prints.)

# the help states the order
"$bin" help 2>&1 | grep -q -e '--disk-size > HARPOON_DISK_SIZE > user config > 32G' || fail "help does not state the disk-size order"

# environment and config layers, invalid values, default size (unit test of the resolution API)
S="$ROOT/harpoon/Sources"
mkdir -p "$tmp/unit-src" "$tmp/unit-ns"
cp "$ROOT/harpoon/regression-disk-size-test.swift" "$tmp/unit-src/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$S/RuntimeConfig.swift" "$S/Version.swift" "$S/ForwardTarget.swift" "$S/HarpoonRuntimeState.swift" "$tmp/unit-src/main.swift" -framework Virtualization -o "$tmp/unit" 2>&1 | grep -E "error" && fail "unit test did not compile" || true
HARPOON_TEST_TMPDIR="$tmp/unit-ns" "$tmp/unit"
echo "disk size regression PASS"
