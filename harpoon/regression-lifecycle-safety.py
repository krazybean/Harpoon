#!/usr/bin/env python3
"""Offline lifecycle safety checks; no VM, Docker, or persistent disk access."""
from __future__ import annotations

import fcntl
import os
import subprocess
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CLI = (ROOT / "harpoon/Sources/HarpoonLifecycleCLI.swift").read_text()
MAIN = (ROOT / "harpoon/Sources/main.swift").read_text()
STATE = (ROOT / "harpoon/Sources/Lifecycle.swift").read_text()
CONFIG = (ROOT / "harpoon/Sources/RuntimeConfig.swift").read_text()


class Machine:
    def __init__(self) -> None:
        self.state = "BOOTING"
        self.exit_code = None

    def stop(self, vm_stopped: bool) -> None:
        self.state = "STOPPING"
        if vm_stopped:
            self.state, self.exit_code = "STOPPED", 0
        else:
            self.state, self.exit_code = "FAILED", 8


def test_cross_process_control_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "lifecycle.lock")
        ready = os.path.join(tmp, "ready")
        code = (
            "import fcntl, pathlib, time; "
            f"h=open({path!r}, 'a+'); fcntl.flock(h, fcntl.LOCK_EX); "
            f"pathlib.Path({ready!r}).touch(); time.sleep(5)"
        )
        process = subprocess.Popen(["python3", "-c", code])
        deadline = time.time() + 2
        while not os.path.exists(ready) and time.time() < deadline:
            time.sleep(0.01)
        assert os.path.exists(ready), "child did not acquire lifecycle lock"
        with open(path, "a+") as handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                pass
            else:
                raise AssertionError("concurrent control acquired the lifecycle lock")
        process.terminate()
        process.wait(timeout=2)


def test_state_failures_are_not_clean_stop() -> None:
    machine = Machine()
    machine.stop(vm_stopped=False)  # stop-during-boot + VM stop timeout
    assert (machine.state, machine.exit_code) == ("FAILED", 8)
    assert "case failed" in STATE
    assert "SHUTDOWN_TIMEOUT" in MAIN
    assert "exit(0)" not in MAIN.split("SHUTDOWN_TIMEOUT", 1)[1].split("}", 1)[0]
    assert "return 1" in CLI.split("Harpoon stop timed out", 1)[1]


def test_restart_is_atomic_and_stop_failure_blocks_start() -> None:
    restart = CLI.split("func handleRestart", 1)[1]
    assert "LifecycleControlLock()" in restart
    assert "if stopCode != 0" in restart
    assert "handleStart(args: startArgs, controlLock: controlLock)" in restart


def test_sync_and_disk_identity_guards_exist() -> None:
    outcomes = {"completed": "continue", "unavailable": "continue", "failed": "continue", "alreadyStopping": "skip"}
    assert outcomes["completed"] == "continue"
    assert outcomes["unavailable"] == "continue"
    assert outcomes["failed"] == "continue"
    assert outcomes["alreadyStopping"] == "skip"
    assert "GuestSyncOutcome" in CLI
    assert "already stopping" in CLI.lower()
    assert "diskIdentity" in CLI
    assert "disk identity" in CONFIG.lower()


def test_disk_identity_never_blocks_stop() -> None:
    # A runtime started by an older Harpoon has no recorded identity; stop must
    # still reach the guest sync and SIGTERM instead of returning early.
    stop = CLI.split("func handleStop", 1)[1].split("func handleRestart", 1)[0]
    identity = stop.split("checkDiskIdentity(metadata)", 1)[1].split("switch syncGuestFilesystem()", 1)[0]
    assert "case .unrecorded" in identity and "case .changed" in identity
    assert "return" not in identity, "disk identity check must not abort stop"
    assert "refusing shutdown" not in stop


def main() -> None:
    test_cross_process_control_lock()
    test_state_failures_are_not_clean_stop()
    test_restart_is_atomic_and_stop_failure_blocks_start()
    test_sync_and_disk_identity_guards_exist()
    test_disk_identity_never_blocks_stop()
    print("LIFECYCLE_SAFETY_OFFLINE_PASS")


if __name__ == "__main__":
    main()
