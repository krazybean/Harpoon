#!/bin/sh
# Disk expansion (user-confirmed): `harpoon disk expand`, `disk status --json` and `disk resize` through the binary in private
# HARPOON_TEST_TMPDIR namespaces, plus a unit test of the policy (thresholds, suggestions, refusals, record, template protection).
# No VM is started, stopped or resized: every case has a stopped namespace, or a stand-in "running" process that is only ever
# checked for being left alone (an unconfirmed or refused expansion must not signal it). Usage: sh harpoon/regression-disk-expand.sh [binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
fake_pid=
cleanup() { [ -z "$fake_pid" ] || kill "$fake_pid" 2>/dev/null || true; rm -rf "$tmp"; }
trap cleanup EXIT
export HARPOON_TEST_ROOT="$tmp/template.img"
head -c 1048576 /dev/zero > "$HARPOON_TEST_ROOT"
unset HARPOON_DISK HARPOON_DISK_SIZE HARPOON_DISK_MAX HARPOON_DISK_HOST_RESERVE HARPOON_KERNEL HARPOON_INITRAMFS HARPOON_CPUS HARPOON_MEMORY_MIB
export HARPOON_DISK_HOST_RESERVE=1M
fail() { echo "disk expand FAIL: $*" >&2; exit 1; }
G=1073741824
n=0
# a fresh namespace holding a disk of the given size (sparse)
ns() { n=$((n + 1)); export HARPOON_TEST_TMPDIR="$tmp/ns$n"; mkdir -p "$HARPOON_TEST_TMPDIR/data"; disk="$HARPOON_TEST_TMPDIR/data/harpoon-root.img"; [ -z "${1:-}" ] || truncate -s "$1" "$disk"; }
sig() { printf "%s %s %s" "$(stat -f %z "$1")" "$(stat -f %m "$1")" "$(head -c 65536 "$1" | shasum | cut -c1-40)"; }
run() { set +e; out=$("$bin" "$@" 2>&1); code=$?; set -e; }
# the same with one extra VAR=value for this call only (a plain `VAR=x run ...` would persist in a POSIX shell function call)
runenv() { v=$1; shift; set +e; out=$(env "$v" "$bin" "$@" 2>&1); code=$?; set -e; }

# --- stopped Harpoon -------------------------------------------------------------------------------------------------------------
# without --yes nothing changes, nothing is prompted, and the exit status says so
ns 4G; before=$(sig "$disk")
run disk expand 8G </dev/null
[ "$code" = 2 ] || fail "unconfirmed: exit $code ($out)"
case "$out" in *"not confirmed: nothing was changed"*) ;; *) fail "unconfirmed: $out";; esac
[ "$(sig "$disk")" = "$before" ] || fail "an unconfirmed expansion changed the disk"
[ ! -e "$HARPOON_TEST_TMPDIR/disk-expansion.json" ] || fail "an unconfirmed expansion left a record"

# --yes on a stopped Harpoon grows the disk and starts nothing
run disk expand 8G --yes
[ "$code" = 0 ] || fail "stopped expand: exit $code ($out)"
[ "$(stat -f %z "$disk")" = $((8 * G)) ] || fail "stopped expand did not grow the disk to 8G"
case "$out" in *"Nothing is started"*) ;; *) fail "stopped expand does not say nothing is started: $out";; esac
[ ! -e "$HARPOON_TEST_TMPDIR/runtime.pid" ] || fail "a stopped expand started something"
[ ! -e "$HARPOON_TEST_TMPDIR/disk-expansion.json" ] || fail "a finished expand left a record"
run status --json; case "$out" in *'"state" : "stopped"'*|*'"state":"stopped"'*) ;; *) fail "state after a stopped expand: $out";; esac

# shrink and no-op are refused, whatever --yes says, and the disk is untouched
before=$(sig "$disk")
run disk expand 4G --yes; [ "$code" = 1 ] || fail "shrink: exit $code"; case "$out" in *"shrinking is not supported"*) ;; *) fail "shrink: $out";; esac
run disk expand 8G --yes; [ "$code" = 1 ] || fail "same size: exit $code"; case "$out" in *"nothing to grow"*) ;; *) fail "same size: $out";; esac
run disk expand 1G --yes; [ "$code" = 1 ] || fail "below minimum: exit $code"
run disk expand banana --yes; [ "$code" = 2 ] || fail "invalid size: exit $code"
run disk expand; [ "$code" = 2 ] || fail "no size: exit $code"
[ "$(sig "$disk")" = "$before" ] || fail "a refused expansion changed the disk"

# the growth cap and the host reserve
ns 4G; before=$(sig "$disk")
runenv HARPOON_DISK_MAX=6G disk expand 8G --yes
[ "$code" = 1 ] || fail "over the cap: exit $code"; case "$out" in *"growth cap"*) ;; *) fail "cap: $out";; esac
runenv HARPOON_DISK_HOST_RESERVE=4000G disk expand 8G --yes
[ "$code" = 1 ] || fail "insufficient host space: exit $code ($out)"; case "$out" in *"not enough host space"*) ;; *) fail "host space: $out";; esac
[ "$(sig "$disk")" = "$before" ] || fail "a refused expansion changed the disk"

# an existing 32 GiB disk keeps its size and is never offered a smaller one
ns 32G; before=$(sig "$disk")
run status --json; run disk status --json
case "$out" in *'"logicalBytes":34359738368'*) ;; *) fail "32G disk not reported as 32G: $out";; esac
case "$out" in *'"suggestedSizes":[68719476736'*) ;; *) fail "32G disk is not offered 64G first: $out";; esac
run disk expand 16G --yes; [ "$code" = 1 ] || fail "shrinking a 32G disk: exit $code"
[ "$(sig "$disk")" = "$before" ] || fail "an existing 32G disk was changed"

# the template is protected: a disk that is the template (symlink) is refused by resize and expand
ns; ln -s "$HARPOON_TEST_ROOT" "$disk"; tsig=$(sig "$HARPOON_TEST_ROOT")
run disk expand 4G --yes; [ "$code" = 1 ] || fail "expand on a template symlink: exit $code ($out)"; case "$out" in *"immutable guest template"*) ;; *) fail "template not named: $out";; esac
run disk resize 4G;       [ "$code" = 1 ] || fail "resize on a template symlink: exit $code ($out)"; case "$out" in *"immutable guest template"*) ;; *) fail "template not named: $out";; esac
[ "$(sig "$HARPOON_TEST_ROOT")" = "$tsig" ] || fail "the template was changed"

# status --json: facts, level and suggestions for a stopped Harpoon (no guest numbers without a running VM)
ns 4G
run disk status --json; [ "$code" = 0 ] || fail "status --json: exit $code"
for k in '"backingPath"' '"logicalBytes":4294967296' '"physicalBytes"' '"pressure":"unknown"' '"suggestedSizes"' '"hostAvailableBytes"' '"maxBytes"' '"hostReserveBytes"' '"expansionRequiresRestart":true'; do
  case "$out" in *"$k"*) ;; *) fail "status --json lacks $k: $out";; esac
done
case "$out" in *'"guest"'*) fail "status --json invented guest numbers for a stopped Harpoon";; esac
# the plain text form still starts with the same lines and creates nothing
ns; run disk status; case "$out" in *"(not yet provisioned)"*"(default 4G)"*) ;; *) fail "unprovisioned status: $out";; esac; [ ! -e "$disk" ] || fail "status created a disk"
run disk status --bogus; [ "$code" = 2 ] || fail "bad status flag: exit $code"

# interrupted expansion: a leftover record is explained by status and doctor, with the recovery step; nothing is rolled back
ns 8G
printf '{"phase":"growing","fromBytes":4294967296,"toBytes":8589934592,"diskPath":"%s","startedAt":"t"}\n' "$disk" > "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run disk status; case "$out" in *"Interrupted disk expansion (growing"*"harpoon start"*) ;; *) fail "status does not explain the interrupted expansion: $out";; esac
run disk status --json; case "$out" in *'"pendingExpansion"'*) ;; *) fail "json lacks pendingExpansion: $out";; esac
case "$out" in *olled*) fail "claims a rollback: $out";; esac
[ "$(stat -f %z "$disk")" = $((8 * G)) ] || fail "reading the record changed the disk"

# --- a "running" Harpoon: a stand-in process is never signalled unless the user confirmed -----------------------------------------
ns 4G
cp /bin/sleep "$HARPOON_TEST_TMPDIR/harpoon-standin"; codesign --force -s - "$HARPOON_TEST_TMPDIR/harpoon-standin" 2>/dev/null  # a copied system binary needs its own ad-hoc signature to run
"$HARPOON_TEST_TMPDIR/harpoon-standin" 600 & fake_pid=$!
echo "$fake_pid" > "$HARPOON_TEST_TMPDIR/runtime.pid"
printf '{"pid":%s,"startedAt":"t","cpus":2,"memoryMiB":2048,"diskPath":"%s","socketPath":"%s/harpoon-docker.sock","uuid":"u","binary":"b"}\n' "$fake_pid" "$disk" "$HARPOON_TEST_TMPDIR" > "$HARPOON_TEST_TMPDIR/runtime.json"
: > "$HARPOON_TEST_TMPDIR/harpoon-docker.sock"; echo HARPOON_RUNNING > "$HARPOON_TEST_TMPDIR/harpoon.log"
run status --json; case "$out" in *'"running"'*) ;; *) fail "stand-in does not look like a running Harpoon: $out";; esac
before=$(sig "$disk")
# valid start settings (fake kernel and initramfs with a matching manifest) so the run reaches the confirmation gate
mkdir -p "$tmp/guest"; echo k > "$tmp/guest/Image-virt"; echo i > "$tmp/guest/initramfs"
printf 'harpoon-guest-assets-v1\nkernel_sha256 %s\ninitramfs_sha256 %s\n' "$(shasum -a 256 "$tmp/guest/Image-virt" | cut -d' ' -f1)" "$(shasum -a 256 "$tmp/guest/initramfs" | cut -d' ' -f1)" > "$tmp/guest/guest-assets.manifest"
export HARPOON_KERNEL="$tmp/guest/Image-virt" HARPOON_INITRAMFS="$tmp/guest/initramfs"
run disk expand 8G </dev/null
[ "$code" = 2 ] || fail "running, unconfirmed: exit $code ($out)"
case "$out" in *"Running containers are interrupted"*) ;; *) fail "running, unconfirmed does not warn about interruption: $out";; esac
case "$out" in *"not confirmed"*) ;; *) fail "running, unconfirmed: $out";; esac
kill -0 "$fake_pid" 2>/dev/null || fail "an unconfirmed expansion signalled the running process"
[ -e "$HARPOON_TEST_TMPDIR/runtime.pid" ] && [ ! -e "$HARPOON_TEST_TMPDIR/harpoon-stop" ] || fail "an unconfirmed expansion touched the lifecycle files"
[ "$(sig "$disk")" = "$before" ] || fail "an unconfirmed expansion changed the disk"
# a start that would be rejected is refused BEFORE anything is stopped, even with --yes
runenv HARPOON_KERNEL="$tmp/guest/missing" disk expand 8G --yes
[ "$code" = 2 ] || fail "running, start would fail: exit $code ($out)"; case "$out" in *"refused before stopping anything"*) ;; *) fail "running, start would fail: $out";; esac
kill -0 "$fake_pid" 2>/dev/null || fail "a refused expansion signalled the running process"
[ "$(sig "$disk")" = "$before" ] || fail "a refused expansion changed the disk"
# the running Harpoon's disk must be the disk being grown
sed -i '' "s|\"diskPath\":\"[^\"]*\"|\"diskPath\":\"$tmp/elsewhere.img\"|" "$HARPOON_TEST_TMPDIR/runtime.json"; : > "$tmp/elsewhere.img"
run disk expand 8G --yes
[ "$code" = 1 ] || fail "different active disk: exit $code ($out)"; case "$out" in *"not "*"refusing to expand a disk it does not use"*|*"refusing to expand a disk it does not use"*) ;; *) fail "different active disk: $out";; esac
kill -0 "$fake_pid" 2>/dev/null || fail "a refused expansion signalled the running process"
kill "$fake_pid" 2>/dev/null || true; fake_pid=

# --- unit test of the policy ---------------------------------------------------------------------------------------------------
unset HARPOON_KERNEL HARPOON_INITRAMFS
S="$ROOT/harpoon/Sources"
mkdir -p "$tmp/unit-src" "$tmp/unit-ns"
cp "$ROOT/harpoon/regression-disk-capacity-test.swift" "$tmp/unit-src/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$S/RuntimeConfig.swift" "$S/Version.swift" "$S/ForwardTarget.swift" "$S/HarpoonRuntimeState.swift" "$S/DiskCapacity.swift" "$tmp/unit-src/main.swift" -framework Virtualization -o "$tmp/unit" 2>&1 | grep -E "error" && fail "unit test did not compile" || true
HARPOON_TEST_TMPDIR="$tmp/unit-ns" "$tmp/unit"
echo "disk expand regression PASS"
