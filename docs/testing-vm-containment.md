# Containing VM-backed tests

Production Harpoon VMs have **no uptime limit** and nothing here changes that. This is only about automated and scripted tests that boot
(or drive) a real VM. Every such script is listed in `tools/vm-backed-tests.txt` and re-runs itself under `tools/vm-test-guard.py`
(`. tools/vm-guard-reexec.sh` as its second line). `harpoon/regression-vm-test-guard.sh` fails if a script that starts the engine or
talks to its Docker endpoint is in neither list.

## What the guard does

| Concern | Mechanism |
|---|---|
| Deadlines | `--deadline` (default 1800 s, `VMGUARD_DEADLINE`) covers startup + execution. Then `--shutdown-grace` (60 s: stop file, then SIGTERM) and `--cleanup-grace` (30 s after SIGKILL to confirm exit). Exit 124 on expiry. |
| Independent watchdog | The guard is a separate process; it never waits on Harpoon's main queue. The stop file and SIGTERM both need that queue, so the last rung is SIGKILL. A detached **reaper** (own session and process group, no inherited lock descriptors) acts if the guard dies or overruns its hard deadline; the guard restarts the reaper if it dies. |
| Ownership | A random per-run marker (`HARPOON_VMGUARD_ID`) is put in the command's environment and inherited by everything it starts, including engines that detach and engines in a namespace the test chose itself (its own `HARPOON_TEST_TMPDIR`). Owned = marker holders ∪ the command's process group (members started after the guard) ∪ engines in the namespace's `runtime.json` that started after the guard. The command is **gated**: it blocks on a pipe until its identity is in the ledger, so a guard that dies first never leaves an unrecorded command running. |
| Exact identity | Every signal targets `(pid, microsecond start time)` from libproc, re-verified immediately before the kill, so a reused pid is never signalled. Never signalled: pid <= 1, the guard, **any ancestor** (the GitHub Actions runner), anything matching runner process names. |
| Production is exclusive | A production-namespace run takes an exclusive lock and **refuses (exit 75)** if another test run holds it, if `/tmp/harpoon.lock` is held, or if the production `runtime.json` names a live engine. `--adopt-existing` / `VMGUARD_ADOPT_EXISTING=1` runs beside a pre-existing engine anyway; that engine stays unowned and is never touched. |
| Admission | Current conditions only: free disk >= 15 GiB, memory-pressure level 1 (normal), and no swapping **right now** (< 64 MiB of swap-out or swap growth in a 3 s window). Swap that is merely *occupied* does not refuse a calm host: macOS leaves swap files allocated long after the pressure ends and grows them on demand. |
| Sampling | Every 15 s (disk, `kern.memorystatus_vm_pressure_level`, `vm.swapusage`). Aborts (exit 70) on free disk < 8 GiB, critical pressure for 3 consecutive samples, or swap growing >= 256 MiB per sample for 3 samples **while pressure is >= 2 at that moment**. A VM growing swap on a calm host is not an abort. |
| Verification | After cleanup every owned process must be dead and no *new* `com.apple.Virtualization.VirtualMachine` helper may remain, otherwise exit 71 even if the test passed. Helpers are reported, never killed. |
| Diagnostics | On failure, one file capped at 256 KiB: reason, owned engines, last 20 samples, last 16 KiB of `harpoon.log`, the serial logs and `runtime.json`. Logs are never copied whole, truncated or deleted. |
| Interruption | SIGTERM/SIGINT/SIGHUP to the guard run the same cleanup (exit 128+n). `tools/vm-test-guard.py --sweep` (run at the start of every guarded run and in an `if: always()` step of the self-hosted workflows) cleans what a dead guard left behind. |

Exit codes: the test's own, `124` deadline, `75` refused, `70` aborted on host pressure, `71` cleanup unverified.

Consequence for the self-hosted stage-7/8 jobs and manual runs: a Harpoon engine that is already running in the production namespace makes
the run refuse instead of reusing it. Stop it first, or set `VMGUARD_ADOPT_EXISTING=1` knowing the test then shares that VM.

## What cannot be guaranteed

- If the guard **and** its reaper are both SIGKILLed (or the machine loses power), nothing acts until the next guarded run or the
  workflow's `--sweep` step. The guard replaces a dead reaper within a poll, but cannot cover a simultaneous double kill. The GitHub
  runner's own orphan cleanup may also kill the reaper, which inherits the job environment.
- **`ps -E` cannot show the environment of Apple platform binaries** (`/bin/sh`, `/bin/sleep`, ...). The marker therefore finds engines,
  Python, Node and other non-platform processes anywhere, but a detached *Apple* binary that leaves the command's process group
  (`setsid`) is invisible to the guard. The ad-hoc signed Harpoon binary was checked: its environment is visible.
- A process that scrubs its environment (`env -i`) **and** leaves the process group escapes.
- Signalling by pid has an unavoidable check-then-kill gap (no pidfd on macOS). Identity is verified immediately before the kill; the
  residual window is microseconds and needs a pid reused with an identical microsecond start time.
- VZ helper processes are launched by launchd, not by Harpoon, and cannot be attributed to a run with certainty. The guard relies on
  them exiting when their engine dies and reports (does not kill) any new helper that outlives the run; a VM another user session
  starts during the window is a false positive.
- SIGKILL on a process in uninterruptible kernel sleep does not take effect until it leaves that state; the run then exits 71.
- Sampling is every 15 s: a shorter spike is invisible, and an abort takes up to one interval plus the cleanup grace. The thresholds are
  policy for a development Mac, not measurements; tune with the `--min-disk-*`, `--swap-growth` and `--admission-window` flags.
- Test-only hooks (`VMGUARD_TEST=1`: fake metrics, engine/helper name match, no-op SIGKILL, spawn pause, ledger/diag locations) are
  inert unless that variable is set. The real escalation against a live engine has not been exercised (no VM was started).
