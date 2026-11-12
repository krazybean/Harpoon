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
case "$out" in *"\"diskLogicalBytes\" : $((8 * G))"*) ;; *) fail "status --json lacks the logical backing size: $out";; esac

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
# A stand-in whose executable path contains "harpoon", as isHarpoonProcess requires. It is built here rather than copied from
# /bin/sleep: a copied system binary loses its platform signature and is killed on the hosted runner.
mkdir -p "$tmp/standin-src"; echo 'import Foundation; sleep(600)' > "$tmp/standin-src/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$tmp/standin-src/main.swift" -o "$HARPOON_TEST_TMPDIR/harpoon-standin" 2>&1 | grep -E "error" && fail "stand-in did not compile" || true
"$HARPOON_TEST_TMPDIR/harpoon-standin" & fake_pid=$!
sleep 1; kill -0 "$fake_pid" 2>/dev/null || fail "the stand-in process died at startup"
# valid start settings (fake kernel and initramfs with a matching manifest) so a run reaches the confirmation gate
mkdir -p "$tmp/guest" "$tmp/guest2"
for d in guest guest2; do
  echo "k$d" > "$tmp/$d/Image-virt"; echo "i$d" > "$tmp/$d/initramfs"
  printf 'harpoon-guest-assets-v1\nkernel_sha256 %s\ninitramfs_sha256 %s\n' "$(shasum -a 256 "$tmp/$d/Image-virt" | cut -d' ' -f1)" "$(shasum -a 256 "$tmp/$d/initramfs" | cut -d' ' -f1)" > "$tmp/$d/guest-assets.manifest"
done
export HARPOON_KERNEL="$tmp/guest/Image-virt" HARPOON_INITRAMFS="$tmp/guest/initramfs"
echo "$fake_pid" > "$HARPOON_TEST_TMPDIR/runtime.pid"
meta() { # meta [cpus] [extra-json]: what the running VM recorded when it started
  printf '{"pid":%s,"startedAt":"t","cpus":%s,"memoryMiB":2048,"diskPath":"%s","socketPath":"%s/harpoon-docker.sock","uuid":"u","binary":"b"%s}\n' "$fake_pid" "${1:-2}" "$disk" "$HARPOON_TEST_TMPDIR" "${2-,\"kernelPath\":\"$HARPOON_KERNEL\",\"initramfsPath\":\"$HARPOON_INITRAMFS\"}" > "$HARPOON_TEST_TMPDIR/runtime.json"
}
meta
: > "$HARPOON_TEST_TMPDIR/harpoon-docker.sock"; echo HARPOON_RUNNING > "$HARPOON_TEST_TMPDIR/harpoon.log"
run status --json; case "$out" in *'"running"'*) ;; *) fail "stand-in does not look like a running Harpoon: $out";; esac
before=$(sig "$disk")
untouched() { kill -0 "$fake_pid" 2>/dev/null || fail "$1: the running process was signalled"; [ ! -e "$HARPOON_TEST_TMPDIR/harpoon-stop" ] || fail "$1: a stop was requested"; [ "$(sig "$disk")" = "$before" ] || fail "$1: the disk changed"; }

run disk expand 8G </dev/null
[ "$code" = 2 ] || fail "running, unconfirmed: exit $code ($out)"
case "$out" in *"Running containers are interrupted"*) ;; *) fail "running, unconfirmed does not warn about interruption: $out";; esac
case "$out" in *"not confirmed"*) ;; *) fail "running, unconfirmed: $out";; esac
[ -e "$HARPOON_TEST_TMPDIR/runtime.pid" ] || fail "an unconfirmed expansion touched the lifecycle files"
[ ! -e "$HARPOON_TEST_TMPDIR/disk-expansion.json" ] || fail "an unconfirmed expansion wrote a record"
untouched "unconfirmed"

# a start that would be rejected is refused BEFORE anything is stopped, even with --yes (here the kernel differs and is missing)
runenv HARPOON_KERNEL="$tmp/guest/missing" disk expand 8G --yes
[ "$code" = 1 ] || fail "running, kernel differs: exit $code ($out)"; case "$out" in *"would not reproduce the running VM"*"kernel:"*) ;; *) fail "running, kernel differs: $out";; esac
untouched "kernel differs"
# the restart must reproduce the running VM: CPUs, memory, kernel and initramfs may not differ, and unrecorded ones cannot be guaranteed
printf '{"cpus":4}\n' > "$HARPOON_TEST_TMPDIR/config.json"
run disk expand 8G --yes
[ "$code" = 1 ] || fail "running, config would change the cpus: exit $code ($out)"; case "$out" in *"cpus: running with 2, a restart would use 4"*"nothing was stopped"*) ;; *) fail "cpus: $out";; esac
untouched "cpus differ"
printf '{"cpus":2,"memory":1024}\n' > "$HARPOON_TEST_TMPDIR/config.json"
run disk expand 8G --yes
[ "$code" = 1 ] || fail "running, config would change the memory: exit $code ($out)"; case "$out" in *"memory: running with 2048 MiB, a restart would use 1024 MiB"*) ;; *) fail "memory: $out";; esac
untouched "memory differs"
rm -f "$HARPOON_TEST_TMPDIR/config.json"
runenv HARPOON_KERNEL="$tmp/guest2/Image-virt" disk expand 8G --yes
[ "$code" = 1 ] || fail "running, other kernel: exit $code ($out)"; case "$out" in *"kernel: running with"*) ;; *) fail "other kernel: $out";; esac
untouched "other kernel"
runenv HARPOON_INITRAMFS="$tmp/guest2/initramfs" disk expand 8G --yes
[ "$code" = 1 ] || fail "running, other initramfs: exit $code ($out)"; case "$out" in *"initramfs: running with"*) ;; *) fail "other initramfs: $out";; esac
untouched "other initramfs"
runenv HARPOON_DISK="$tmp/other-disk.img" disk expand 8G --yes
[ "$code" = 1 ] || fail "running, other disk: exit $code ($out)"; case "$out" in *"disk: running with"*) ;; *) fail "other disk: $out";; esac
untouched "other disk"
meta 2 ""   # a VM started by an older build recorded no kernel or initramfs
run disk expand 8G --yes
[ "$code" = 1 ] || fail "running, unrecorded kernel: exit $code ($out)"; case "$out" in *"not recorded by the Harpoon that started this VM"*) ;; *) fail "unrecorded: $out";; esac
untouched "unrecorded kernel"
meta   # restore

# the recovery record must be on disk before anything is stopped: a record that cannot be written refuses the expansion
mkdir "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run disk expand 8G --yes
[ "$code" = 1 ] || fail "running, record not writable: exit $code ($out)"; case "$out" in *"cannot write the recovery record"*"nothing was stopped or resized"*) ;; *) fail "record: $out";; esac
untouched "unwritable record"
rmdir "$HARPOON_TEST_TMPDIR/disk-expansion.json"

# the running Harpoon's disk must be the disk being grown
sed -i '' "s|\"diskPath\":\"[^\"]*\"|\"diskPath\":\"$tmp/elsewhere.img\"|" "$HARPOON_TEST_TMPDIR/runtime.json"; : > "$tmp/elsewhere.img"
run disk expand 8G --yes
[ "$code" = 1 ] || fail "different active disk: exit $code ($out)"; case "$out" in *"refusing to expand a disk it does not use"*) ;; *) fail "different active disk: $out";; esac
untouched "different active disk"
meta

# an expansion that is running is visible to status, the other lifecycle commands and the desktop; an interrupted one is not "active"
printf '{"phase":"growing","fromBytes":4294967296,"toBytes":8589934592,"diskPath":"%s","startedAt":"t","pid":%s}\n' "$disk" "$fake_pid" > "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run status --json; case "$out" in *'"active" : true'*) ;; *) fail "status does not show the running expansion: $out";; esac
run disk status; case "$out" in *"Disk expansion in progress (growing"*) ;; *) fail "disk status does not show the running expansion: $out";; esac
perl -e 'open my $f, ">", $ARGV[0] or die; flock($f, 2) or die; print "held\n"; sleep 30' "$HARPOON_TEST_TMPDIR/harpoon-lifecycle.lock" > "$tmp/holder.out" &
holder=$!; sleep 1
for c in start stop restart; do
  run $c; [ "$code" = 11 ] || fail "$c during an expansion: exit $code ($out)"; case "$out" in *"disk expansion is in progress (growing, 4 GiB -> 8 GiB)"*) ;; *) fail "$c does not name the expansion: $out";; esac
done
run disk expand 8G --yes; [ "$code" = 11 ] || fail "a second expansion: exit $code ($out)"
untouched "competing lifecycle commands"
printf '{"phase":"growing","fromBytes":4294967296,"toBytes":8589934592,"diskPath":"%s","startedAt":"t","pid":99999999}\n' "$disk" > "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run status --json; case "$out" in *'"active" : false'*) ;; *) fail "a dead expansion is shown as active: $out";; esac
run start; [ "$code" = 11 ] || fail "start while the lock is held: exit $code"; case "$out" in *"another lifecycle operation is in progress"*) ;; *) fail "a dead expansion is named as the cause: $out";; esac
kill "$holder" 2>/dev/null || true; wait "$holder" 2>/dev/null || true
rm -f "$HARPOON_TEST_TMPDIR/disk-expansion.json"
kill "$fake_pid" 2>/dev/null || true; fake_pid=

# --- unit test of the policy ---------------------------------------------------------------------------------------------------
unset HARPOON_KERNEL HARPOON_INITRAMFS
S="$ROOT/harpoon/Sources"
mkdir -p "$tmp/unit-src" "$tmp/unit-ns"
cp "$ROOT/harpoon/regression-disk-capacity-test.swift" "$tmp/unit-src/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$S/RuntimeConfig.swift" "$S/Version.swift" "$S/ForwardTarget.swift" "$S/HarpoonRuntimeState.swift" "$S/BridgePair.swift" "$S/DiskCapacity.swift" "$tmp/unit-src/main.swift" -framework Virtualization -o "$tmp/unit" 2>&1 | grep -E "error" && fail "unit test did not compile" || true
HARPOON_TEST_TMPDIR="$tmp/unit-ns" "$tmp/unit"
echo "disk expand regression PASS"
