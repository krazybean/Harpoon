#!/bin/sh
# VM test containment, as behaviour (no VM: fake engines/helpers) plus mutation checks and an inventory check:
#   1. the guard's scenarios pass (deadlines, owned-process cleanup, failed cleanup, stale ownership, concurrency, watchdog/reaper, resources);
#   2. each safeguard, disabled in a copy of the guard, makes the scenarios fail (a test that cannot detect its safeguard fails this script);
#   3. every script that starts or drives a real VM is in tools/vm-backed-tests.txt and (if `guarded`) really re-execs under the guard.
# Usage: sh harpoon/regression-vm-test-guard.sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
fail() { echo "vm test guard FAIL: $*" >&2; exit 1; }
GUARD=tools/vm-test-guard.py
TEST=harpoon/regression-vm-test-guard.py

echo "== scenarios (real guard)"
python3 "$TEST" || fail "scenarios"

echo "== inventory"
n=0
while read -r kind path rest; do
  case "$kind" in guarded|exempt) ;; *) continue ;; esac
  [ -f "$path" ] || fail "manifest lists a missing file: $path"
  if [ "$kind" = guarded ]; then
    sed -n 2p "$path" | grep -q 'vm-guard-reexec.sh' || fail "$path is listed as guarded but line 2 does not source vm-guard-reexec.sh"
    interp=sh; head -1 "$path" | grep -q bash && interp=bash
    VMGUARD_DRYRUN=1 $interp "$path" 2>&1 | grep -q "^VMGUARD_WOULD_GUARD" || fail "$path does not re-exec under the guard"
    n=$((n + 1))
  else
    [ -n "$rest" ] || fail "exempt entry $path has no reason"
  fi
done < tools/vm-backed-tests.txt
echo "  ok   $n guarded scripts re-exec under the guard"
git ls-files '*.sh' | while read -r f; do
  grep -qE '(build/harpoon|BIN"?|\$HARPOON) +start( |$|")|docker --context harpoon|DOCKER_HOST=unix:///tmp/harpoon|harpoon-docker\.sock' "$f" || continue
  grep -qE "^(guarded|exempt) $f( |\$)" tools/vm-backed-tests.txt || echo "$f"
done > "$tmp/unlisted"
[ ! -s "$tmp/unlisted" ] || { cat "$tmp/unlisted" >&2; fail "scripts that start or drive the engine are in neither list of tools/vm-backed-tests.txt"; }
echo "  ok   no unlisted script starts or drives the engine"
for w in stage-7-consistency stage-8-disk-reclamation; do
  grep -q 'vm-test-guard.py --sweep' ".github/workflows/$w.yml" || fail "$w workflow lacks the always() sweep step"
done
echo "  ok   self-hosted workflows sweep orphans in an always() step"

echo "== mutation checks: each disabled safeguard must fail its scenarios"
mutate() { # mutate <label> <old> <new>
  mkdir -p "$tmp/$1"
  python3 - "$GUARD" "$tmp/$1/vm-test-guard.py" "$2" "$3" <<'PY' || fail "mutation '$1' did not apply"
import sys
src, dst, old, new = sys.argv[1:5]
s = open(src).read()
if s.count(old) != 1: sys.exit(1)
open(dst, "w").write(s.replace(old, new))
PY
}
caught() { # caught <label> <scenario...>
  label=$1; shift
  if VMGUARD_BIN="$tmp/$label/vm-test-guard.py" python3 "$TEST" "$@" >"$tmp/$label/out" 2>&1; then cat "$tmp/$label/out" >&2; fail "scenarios still PASS with the safeguard removed: $label"; fi
  echo "  mutation caught: $label"
}
mutate sigkill 'NO_SIGKILL = TEST and os.environ.get("VMGUARD_TEST_SIGKILL_NOOP") == "1"' 'NO_SIGKILL = True'
caught sigkill engine_cleanup
mutate ownership 'start_epoch(i) >= led["created"] - 0.5 and i != preexisting' 'True'
caught ownership production_ownership adopt_existing
mutate deadline 'if now - t0 > a.deadline:' 'if False:'
caught deadline deadline
mutate identity 'return bool(b) and b["start"] == i["start"] and b["status"] != 5' 'return bool(b) and b["status"] != 5'
caught identity pid_identity stale_ledger
mutate sweep 'if not led or alive(led["guard"]): continue' 'continue'
caught sweep stale_ledger
mutate protected 'if PROTECTED.search(command(i["pid"])): return False' 'pass'
caught protected protected
mutate ancestor ' or i["pid"] in ancestors():' ':'
caught ancestor ancestor
mutate reaper 'if over or not alive(led["guard"]):' 'if over:'
caught reaper guard_killed
mutate respawn 'if reaper.poll() is not None:' 'if False:'
caught respawn reaper_respawn
mutate gate 'eval "read go <&$VMGUARD_GO_FD" || exit 125;' 'true;'
caught gate startup_race
mutate marker 'tok = f" HARPOON_VMGUARD_ID={run_id}"' 'tok = f" NEVER_MATCHES_{run_id}"'
caught marker own_namespace
mutate exclusive 'try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)  # EXCLUSIVE_LOCK' 'try: pass'
caught exclusive production_exclusive
mutate preexisting 'if not adopt:' 'if False:'
caught preexisting production_exclusive
mutate adoptflag 'if not adopt:' 'if True:'
caught adoptflag adopt_existing production_ownership
mutate helpers 'elif leaked: problem' 'elif False: problem'
caught helpers helper_leak
mutate verify 'if left: problem = ' 'if False: problem = '
caught verify failed_cleanup
mutate diagcap '.encode("utf-8", "replace")[:DIAG_CAP])' '.encode("utf-8", "replace"))'
caught diagcap diag_bounded
mutate sustained 'if self.crit >= a.pressure_samples' 'if self.crit >= 1'
caught sustained pressure
mutate preflight 'if m1["disk_free"] < a.min_disk_start:' 'if False:'
caught preflight preflight
mutate diskabort 'if m["disk_free"] < a.min_disk_abort:' 'if False:'
caught diskabort pressure
mutate engine_run 'tokens[1:2] == ["run"] and (len(tokens) == 2 or tokens[2].startswith("-"))' 'True'
caught engine_run engine_classification
mutate engine_args 'and (len(tokens) == 2 or tokens[2].startswith("-"))' 'and True'
caught engine_args engine_classification
mutate sigterm 'if state["sig"]: outcome' 'if False: outcome'
caught sigterm interrupt
mutate occupancy 'if moved >= ACTIVE_SWAP:' 'if moved >= ACTIVE_SWAP or m2["swap_used"] / max(m2["swap_total"], 1) >= 0.85:'
caught occupancy preflight
mutate activity 'if moved >= ACTIVE_SWAP:' 'if False:'
caught activity preflight
mutate calm 'if m["pressure"] >= 2 and len(self.swaps)' 'if len(self.swaps)'
caught calm pressure

echo "vm test guard: PASS"
