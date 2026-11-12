#!/bin/sh
# User-confirmed space reclamation: `harpoon disk reclaim` through the binary in private HARPOON_TEST_TMPDIR namespaces, against a
# stand-in "running" process and a FAKE guest management socket (python) that records every request and answers `fstrim /` by
# scripted mode. No VM is started, stopped or resized, no real guest is asked anything, and Docker is not involved: the fake
# "reclaim" simulates a discard by rewriting a scratch file sparse. Usage: sh harpoon/regression-disk-reclaim.sh [binary]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
bin=${1:-${HARPOON_BIN:-$ROOT/harpoon/build/harpoon}}
tmp=$(mktemp -d)
fake_pid=; mgmt_pid=; holder=
cleanup() { for p in "$fake_pid" "$mgmt_pid" "$holder"; do [ -z "$p" ] || kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"; }
trap cleanup EXIT
export HARPOON_TEST_ROOT="$tmp/template.img"
head -c 1048576 /dev/zero > "$HARPOON_TEST_ROOT"
unset HARPOON_DISK HARPOON_DISK_SIZE HARPOON_DISK_MAX HARPOON_DISK_HOST_RESERVE HARPOON_KERNEL HARPOON_INITRAMFS HARPOON_CPUS HARPOON_MEMORY_MIB
fail() { echo "disk reclaim FAIL: $*" >&2; exit 1; }
G=1073741824
run() { set +e; out=$("$bin" "$@" 2>&1); code=$?; set -e; }
phys() { echo $(( $(stat -f %b "$1") * 512 )); }
has() { case "$out" in *"$1"*) ;; *) fail "$2: expected '$1' in: $out";; esac; }

export HARPOON_TEST_TMPDIR="$tmp/ns"; mkdir -p "$HARPOON_TEST_TMPDIR/data"
disk="$HARPOON_TEST_TMPDIR/data/harpoon-root.img"
truncate -s 4G "$disk"
dd if=/dev/urandom of="$disk" bs=1048576 count=64 conv=notrunc 2>/dev/null   # 64 MiB allocated in a 4 GiB sparse file
logical=$(stat -f %z "$disk"); [ "$logical" = $((4 * G)) ] || fail "fixture is not 4G"

# --- a stopped Harpoon cannot be asked to trim ----------------------------------------------------------------------------------
run disk reclaim --yes
[ "$code" = 1 ] || fail "stopped: exit $code ($out)"; has "needs the running guest" "stopped"; has "nothing was changed" "stopped"
[ "$(phys "$disk")" -gt 0 ] || fail "fixture lost its data"

# --- a stand-in running process (named harpoon-*, as isHarpoonProcess requires; not a copied system binary) ------------------------
mkdir -p "$tmp/standin-src"; echo 'import Foundation; sleep(600)' > "$tmp/standin-src/main.swift"
xcrun swiftc -module-cache-path "$tmp/module-cache" "$tmp/standin-src/main.swift" -o "$HARPOON_TEST_TMPDIR/harpoon-standin" 2>&1 | grep -E "error" && fail "stand-in did not compile" || true
"$HARPOON_TEST_TMPDIR/harpoon-standin" & fake_pid=$!
sleep 1; kill -0 "$fake_pid" 2>/dev/null || fail "the stand-in process died at startup"
echo "$fake_pid" > "$HARPOON_TEST_TMPDIR/runtime.pid"
printf '{"pid":%s,"startedAt":"t","cpus":2,"memoryMiB":2048,"diskPath":"%s","socketPath":"%s/harpoon-docker.sock","uuid":"u","binary":"b"}\n' "$fake_pid" "$disk" "$HARPOON_TEST_TMPDIR" > "$HARPOON_TEST_TMPDIR/runtime.json"
: > "$HARPOON_TEST_TMPDIR/harpoon-docker.sock"; echo HARPOON_RUNNING > "$HARPOON_TEST_TMPDIR/harpoon.log"
run status --json; has '"running"' "stand-in is not running"

# disconnected engine: running, but no management socket
run disk reclaim --yes
[ "$code" = 1 ] || fail "no mgmt: exit $code ($out)"; has "management service is not reachable" "no mgmt"; has "nothing was changed" "no mgmt"
untouched() { kill -0 "$fake_pid" 2>/dev/null || fail "$1: the running process was signalled"; [ ! -e "$HARPOON_TEST_TMPDIR/harpoon-stop" ] || fail "$1: a stop was requested"; [ "$(stat -f %z "$disk")" = "$logical" ] || fail "$1: the logical size changed"; }

# --- fake guest management service ----------------------------------------------------------------------------------------------
cat > "$tmp/fakemgmt.py" <<'PY'
import json, os, random, socketserver, sys
ns, disk = sys.argv[1], sys.argv[2]
sock = os.path.join(ns, "harpoon-mgmt.sock")
class H(socketserver.StreamRequestHandler):
    def handle(self):
        req = json.loads(self.rfile.readline())
        argv = req.get("argv", [])
        with open(os.path.join(ns, "mgmt.log"), "a") as f: f.write(json.dumps(argv) + "\n")
        mode = open(os.path.join(ns, "mode")).read().strip() if os.path.exists(os.path.join(ns, "mode")) else "ok"
        res = {"exit": 0, "stdout": "", "stderr": ""}
        if argv == ["fstrim", "/"]:
            size = os.path.getsize(disk)
            if mode == "reclaim":      # a discard that frees everything: rewrite the file sparse, same logical size
                os.truncate(disk, 0); os.truncate(disk, size)
            elif mode == "grow":       # the guest kept writing while trimming
                with open(disk, "r+b") as f: f.seek(random.randrange(1024, 3000) * 1024**2); f.write(os.urandom(8 * 1024 * 1024))
            elif mode == "unsupported":
                res = {"exit": 1, "stdout": "", "stderr": "fstrim: /: the discard operation is not supported"}
            elif mode == "fail":
                res = {"exit": 32, "stdout": "", "stderr": "fstrim: /: FITRIM ioctl failed: Input/output error"}
        self.wfile.write((json.dumps(res) + "\n").encode())
class S(socketserver.ThreadingMixIn, socketserver.UnixStreamServer): daemon_threads = True
if os.path.exists(sock): os.unlink(sock)
S(sock, H).serve_forever()
PY
python3 -I "$tmp/fakemgmt.py" "$HARPOON_TEST_TMPDIR" "$disk" & mgmt_pid=$!
i=0; while [ ! -S "$HARPOON_TEST_TMPDIR/harpoon-mgmt.sock" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
[ -S "$HARPOON_TEST_TMPDIR/harpoon-mgmt.sock" ] || fail "fake management service did not start"
echo HARPOON_MGMT_LISTENER_READY >> "$HARPOON_TEST_TMPDIR/harpoon.log"
trims() { grep -c '"fstrim"' "$HARPOON_TEST_TMPDIR/mgmt.log" 2>/dev/null || true; }

# --- without --yes: explains, changes nothing, asks the guest nothing --------------------------------------------------------------
before=$(phys "$disk")
run disk reclaim </dev/null
[ "$code" = 2 ] || fail "unconfirmed: exit $code ($out)"
has "does not delete Docker images, containers or volumes" "unconfirmed"; has "does not restart Harpoon" "unconfirmed"
has "not known in advance" "unconfirmed"; has "Nothing was changed" "unconfirmed"; has "4 GiB" "unconfirmed shows the capacity"
[ "$(trims)" = 0 ] || fail "an unconfirmed reclaim asked the guest to trim"
[ "$(phys "$disk")" = "$before" ] || fail "an unconfirmed reclaim changed the allocation"
untouched "unconfirmed"
run disk reclaim --bogus; [ "$code" = 2 ] || fail "unknown flag: exit $code"

# --- success: actual change is measured and reported, logical size preserved, nothing else is asked -------------------------------
echo reclaim > "$HARPOON_TEST_TMPDIR/mode"; before=$(phys "$disk")
run disk reclaim --yes --json
[ "$code" = 0 ] || fail "reclaim: exit $code ($out)"
has '"status":"reclaimed"' "reclaim json"; has "\"physicalBeforeBytes\":$before" "reclaim before"
after=$(phys "$disk"); has "\"physicalAfterBytes\":$after" "reclaim after"; has "\"reclaimedBytes\":$((before - after))" "reclaimed delta"
[ "$after" -lt "$before" ] || fail "the simulated discard did not shrink the allocation"
has "\"logicalBytes\":$logical" "logical preserved"; untouched "reclaim"
[ "$(trims)" = 1 ] || fail "expected exactly one fstrim, saw $(trims)"
if grep -vE '^\["(true|fstrim)"' "$HARPOON_TEST_TMPDIR/mgmt.log" | grep -q .; then fail "the guest was asked for something other than fstrim: $(cat "$HARPOON_TEST_TMPDIR/mgmt.log")"; fi
grep -q '^\["fstrim", "/"\]$' "$HARPOON_TEST_TMPDIR/mgmt.log" || fail "fstrim was not run on /"
[ ! -e "$HARPOON_TEST_TMPDIR/disk-expansion.json" ] || fail "a reclaim left an expansion record"
# text form
dd if=/dev/urandom of="$disk" bs=1048576 count=16 conv=notrunc 2>/dev/null
run disk reclaim --yes; [ "$code" = 0 ] || fail "reclaim text: exit $code"; has "Reclaimed " "reclaim text"

# --- zero reclaimed is reported as such (not as success of a promise) ---------------------------------------------------------------
echo ok > "$HARPOON_TEST_TMPDIR/mode"; before=$(phys "$disk")
run disk reclaim --yes --json
[ "$code" = 0 ] || fail "zero: exit $code ($out)"; has '"status":"nothing-reclaimed"' "zero"; has '"reclaimedBytes":0' "zero"; has "\"physicalAfterBytes\":$before" "zero unchanged"
run disk reclaim --yes; has "Nothing was reclaimed: the allocation is unchanged" "zero text"

# --- allocation that grew during the trim is reported honestly ----------------------------------------------------------------------
echo grow > "$HARPOON_TEST_TMPDIR/mode"
run disk reclaim --yes --json; [ "$code" = 0 ] || fail "grow: exit $code ($out)"; has '"status":"nothing-reclaimed"' "grow"; has '"reclaimedBytes":0' "grow"
case "$out" in *'"changeBytes":-'*) fail "growth reported as a reduction: $out";; esac
run disk reclaim --yes; has "grew by" "grow text"

# --- unsupported trim and failures: clear, exit 1, nothing deleted, VM untouched ------------------------------------------------------
echo unsupported > "$HARPOON_TEST_TMPDIR/mode"; before=$(phys "$disk")
run disk reclaim --yes --json; [ "$code" = 1 ] || fail "unsupported: exit $code"; has '"status":"unsupported"' "unsupported"; has "does not support trimming" "unsupported msg"
[ "$(phys "$disk")" = "$before" ] || fail "an unsupported trim changed the allocation"; untouched "unsupported"
echo fail > "$HARPOON_TEST_TMPDIR/mode"
run disk reclaim --yes --json; [ "$code" = 1 ] || fail "failed: exit $code"; has '"status":"failed"' "failed"; has "FITRIM ioctl failed" "failed detail"; has "not restarted" "failed msg"
untouched "failed"

# --- concurrent operations: the lifecycle lock refuses, and the guest is not asked ---------------------------------------------------
echo reclaim > "$HARPOON_TEST_TMPDIR/mode"; t0=$(trims)
perl -e 'open my $f, ">", $ARGV[0] or die; flock($f, 2) or die; print "held\n"; sleep 30' "$HARPOON_TEST_TMPDIR/harpoon-lifecycle.lock" > "$tmp/holder.out" &
holder=$!; sleep 1
run disk reclaim --yes; [ "$code" = 11 ] || fail "locked: exit $code ($out)"; has "HARPOON_CONTROL_BUSY" "locked"
printf '{"phase":"growing","fromBytes":4294967296,"toBytes":8589934592,"diskPath":"%s","startedAt":"t","pid":%s}\n' "$disk" "$fake_pid" > "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run disk reclaim --yes --json; [ "$code" = 11 ] || fail "during expansion: exit $code ($out)"; has "disk expansion is in progress" "names the expansion"
[ "$(trims)" = "$t0" ] || fail "a refused reclaim still asked the guest to trim"
kill "$holder" 2>/dev/null || true; wait "$holder" 2>/dev/null || true; holder=
rm -f "$HARPOON_TEST_TMPDIR/disk-expansion.json"
untouched "concurrent"

# --- status --json reports an interrupted expansion with its recovery text, and nothing once it is gone ------------------------------
printf '{"phase":"growing","fromBytes":4294967296,"toBytes":8589934592,"diskPath":"%s","startedAt":"t","pid":99999999}\n' "$disk" > "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run status --json; has '"active" : false' "interrupted"; has '"recovery"' "interrupted has recovery"; has "The disk was not grown" "recovery judged from size"
rm -f "$HARPOON_TEST_TMPDIR/disk-expansion.json"
run status --json; case "$out" in *diskExpansion*) fail "a cleared record still shows in status: $out";; esac

echo "disk reclaim regression PASS"
