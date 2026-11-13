#!/bin/sh
# Host resource safeguards, as behaviour: the serial collector (bounded memory/disk under a console flood), cross-namespace memory
# admission (live vs stale, atomic under concurrent starts) and the guest idle loop (never spins). No VM, no Docker, no engine.
#
# Every safeguard is also MUTATION-CHECKED: a copy of the source with that safeguard disabled is compiled and the same tests must
# FAIL against it. A mutation that does not change the source, does not compile, or does not make the tests fail fails this script,
# so a test that cannot detect its safeguard being removed cannot pass. Usage: sh harpoon/regression-resource-safeguards.sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
SRC="$ROOT/harpoon/Sources"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
fail() { echo "resource safeguards FAIL: $*" >&2; exit 1; }

build() { # build <out> <test.swift> <source...>
  out=$1; test=$2; shift 2
  d=$(dirname "$out"); mkdir -p "$d/m"; cp "$test" "$d/main.swift"
  xcrun swiftc -module-cache-path "$tmp/mc" "$@" "$d/main.swift" -o "$out" >"$d/build.log" 2>&1 || { cat "$d/build.log" >&2; fail "did not compile: $out"; }
}
# mutate <srcfile> <label> <old> <new>  -> writes $tmp/mut/<label>/<basename>, requires exactly one occurrence of <old>
mutate() {
  mkdir -p "$tmp/mut/$2"
  python3 - "$1" "$tmp/mut/$2/$(basename "$1")" "$3" "$4" <<'PY' || fail "mutation '$2' did not apply to $(basename "$1")"
import sys
src, dst, old, new = sys.argv[1:5]
s = open(src).read()
if s.count(old) != 1: sys.exit(1)
open(dst, "w").write(s.replace(old, new))
PY
}

COLLECTOR_TEST="$ROOT/harpoon/regression-serial-collector-test.swift"
ADMISSION_TEST="$ROOT/harpoon/regression-memory-admission-test.swift"

echo "== serial collector (real source)"
build "$tmp/c/t" "$COLLECTOR_TEST" "$SRC/SerialCollector.swift" "$SRC/BridgePair.swift"
"$tmp/c/t" || fail "serial collector tests"
echo "== memory admission (real source)"
build "$tmp/a/t" "$ADMISSION_TEST" "$SRC/MemoryAdmission.swift"
"$tmp/a/t" || fail "memory admission tests"

# --- mutation checks ---------------------------------------------------------------------------------------------------------------
expect_fail() { # expect_fail <label> <test> <mutated-file> <other-sources...>
  label=$1; test=$2; shift 2
  build "$tmp/mut-$label/t" "$test" "$@"
  if "$tmp/mut-$label/t" >"$tmp/mut-$label/out" 2>&1; then
    cat "$tmp/mut-$label/out" >&2; fail "tests still PASS with the safeguard removed: $label"
  fi
  echo "  mutation caught: $label"
}
SC="$SRC/SerialCollector.swift"; MA="$SRC/MemoryAdmission.swift"
echo "== mutation checks: each disabled safeguard must fail the tests"
mutate "$SC" carry     'let window = carry + chunk' 'let window = chunk'
expect_fail carry "$COLLECTOR_TEST" "$tmp/mut/carry/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$SC" rotate    'if size >= maxBytes { // ROTATE' 'if false { // ROTATE'
expect_fail rotate "$COLLECTOR_TEST" "$tmp/mut/rotate/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$SC" budget    'let allowed = min(chunk.count, Int(tokens))' 'let allowed = chunk.count'
expect_fail budget "$COLLECTOR_TEST" "$tmp/mut/budget/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$SC" tailtrim  'if tailBuf.count > tailLimit * 2' 'if false'
expect_fail tailtrim "$COLLECTOR_TEST" "$tmp/mut/tailtrim/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$SC" reasoncap 'cap += chunk.prefix(Self.failedReasonBytes - cap.count)' 'cap += chunk'
expect_fail reasoncap "$COLLECTOR_TEST" "$tmp/mut/reasoncap/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$SC" panicline 'window[i...].contains(10) || window.count - i >= 200' 'true'
expect_fail panicline "$COLLECTOR_TEST" "$tmp/mut/panicline/SerialCollector.swift" "$SRC/BridgePair.swift"
mutate "$MA" lock      'while flock(lockFd, LOCK_EX) != 0 {' 'while false {'
expect_fail admissionlock "$ADMISSION_TEST" "$tmp/mut/lock/MemoryAdmission.swift"
mutate "$MA" stale     'if flock(fd, LOCK_EX | LOCK_NB) == 0 { unlink(path); continue }' 'if false { continue }'
expect_fail stale "$ADMISSION_TEST" "$tmp/mut/stale/MemoryAdmission.swift"
mutate "$MA" live      'if flock(fd, LOCK_EX | LOCK_NB) == 0 { unlink(path); continue }' 'if true { unlink(path); continue }'
expect_fail live "$ADMISSION_TEST" "$tmp/mut/live/MemoryAdmission.swift"
mutate "$MA" overhead  'overheadMiB * (activeMiB.count + 1)' 'overheadMiB'
expect_fail overhead "$ADMISSION_TEST" "$tmp/mut/overhead/MemoryAdmission.swift"

# --- guest idle loop -------------------------------------------------------------------------------------------------------------
echo "== guest init idle loop (real block from tools/guest-builder/src/init)"
INIT="$ROOT/tools/guest-builder/src/init"
sed -n '/^# BEGIN idle-loop/,/^# END idle-loop/p' "$INIT" > "$tmp/idle.raw"
[ -s "$tmp/idle.raw" ] || fail "idle-loop markers not found in init"
mkdir -p "$tmp/shim-fail" "$tmp/shim-ok" "$tmp/shim-once" "$tmp/shim-cap"
printf '#!/bin/sh\necho x >> "$CALLS"\nexit 1\n' > "$tmp/shim-fail/sleep"
printf '#!/bin/sh\necho x >> "$CALLS"\nexec /bin/sleep 0.2\n' > "$tmp/shim-ok/sleep"
printf '#!/bin/sh\nif [ ! -f "$CALLS.once" ]; then : > "$CALLS.once"; echo x >> "$CALLS"; exit 1; fi\necho x >> "$CALLS"\nexec /bin/sleep 0.2\n' > "$tmp/shim-once/sleep"
# fails like shim-fail, and after 50 calls stops the loop that called it (its parent shell): a spin reaches 50 at any speed, a paced loop never does
printf '#!/bin/sh\necho x >> "$CALLS"\n[ "$(wc -l < "$CALLS")" -ge 50 ] && kill "$PPID"\nexit 1\n' > "$tmp/shim-cap/sleep"
chmod +x "$tmp"/shim-*/sleep

# run_idle <script> <shim dir> <seconds> [<tenths of a second to wait for it to stop itself>]: runs the loop for a bounded time, then kills it. Sets CALLS (sleep invocations), $tmp/console.
run_idle() {
  : > "$tmp/console"; : > "$tmp/calls"; rm -f "$tmp/calls.once" "$tmp/fifo"
  # the console is a device in the guest (writes append); here it is a file, so make the writes append too
  sed -e "s#> /dev/hvc0#>> $tmp/console#g" -e "s#/dev/hvc0#$tmp/console#g" "$1" > "$tmp/run.sh"
  CALLS="$tmp/calls" IDLE_FIFO="$tmp/fifo" PATH="$2:$PATH" sh "$tmp/run.sh" 2>/dev/null &
  pid=$!
  if [ -n "${4:-}" ]; then i=0; while kill -0 "$pid" 2>/dev/null && [ "$i" -lt "$4" ]; do /bin/sleep 0.1; i=$((i+1)); done; else /bin/sleep "$3"; fi
  kill "$pid" 2>/dev/null || true; { wait "$pid"; } 2>/dev/null || true
  CALLS_N=$(wc -l < "$tmp/calls" | tr -d ' ')
  FAILED_N=$(grep -c HARPOON_INIT_SLEEP_FAILED "$tmp/console" || true)
  OK_N=$(grep -c HARPOON_SPIKE_OK "$tmp/console" || true)
} 2>/dev/null  # the shell's "Terminated" job notice for a loop the shim stopped

run_idle "$tmp/idle.raw" "$tmp/shim-fail" 3
[ "$CALLS_N" -le 4 ] || fail "idle loop spins when sleep fails: $CALLS_N sleep calls in 3 s"
[ "$FAILED_N" -eq 1 ] || fail "failed sleep must be reported exactly once, got $FAILED_N"
[ "$OK_N" -eq 0 ] || fail "no heartbeat while sleep is failing, got $OK_N"
echo "  ok   failing sleep: $CALLS_N call(s) in 3 s, one HARPOON_INIT_SLEEP_FAILED, no heartbeat flood"

run_idle "$tmp/idle.raw" "$tmp/shim-ok" 2
[ "$OK_N" -ge 3 ] && [ "$FAILED_N" -eq 0 ] || fail "healthy sleep must keep the heartbeat (heartbeats=$OK_N failed=$FAILED_N)"
echo "  ok   healthy sleep: $OK_N heartbeats, nothing reported"

run_idle "$tmp/idle.raw" "$tmp/shim-once" 4
[ "$FAILED_N" -eq 1 ] && [ "$OK_N" -ge 1 ] || fail "one transient failure must recover (failed=$FAILED_N heartbeats=$OK_N)"
echo "  ok   one transient failure is reported once and the heartbeat resumes"

sed 's/-lt 20/-lt 0/' "$tmp/idle.raw" > "$tmp/idle.parked"
cmp -s "$tmp/idle.raw" "$tmp/idle.parked" && fail "parked-mode mutation did not apply"
run_idle "$tmp/idle.parked" "$tmp/shim-fail" 2
[ "$CALLS_N" -le 1 ] || fail "parked loop must block, not retry sleep: $CALLS_N calls in 2 s"
echo "  ok   after repeated failure PID 1 parks on a blocking read ($CALLS_N sleep call)"

# negative control: the loop this replaced must be seen to spin by the same harness
printf 'while true; do sleep 2; echo "HARPOON_SPIKE_OK" > /dev/hvc0; done\n' > "$tmp/idle.old"
# (count-based, not rate-based: the shim ends the loop at 50 calls, so a slow host only takes longer, up to the 30 s wait)
run_idle "$tmp/idle.old" "$tmp/shim-cap" 0 300
[ "$CALLS_N" -ge 50 ] || fail "harness cannot detect the old spin (only $CALLS_N calls before the 30 s limit): the idle-loop checks above prove nothing"
echo "  ok   negative control: the old loop reached the $CALLS_N-call cap"

echo "resource safeguards: PASS"
