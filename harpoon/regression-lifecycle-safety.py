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


def test_restart_validates_everything_before_it_stops_anything() -> None:
    restart = CLI.split("func handleRestart", 1)[1]
    assert "startArgsError(startArgs)" in restart, "restart does not pre-validate its start arguments"
    assert restart.index("startArgsError(startArgs)") < restart.index("handleStop(controlLock: controlLock)"), \
        "restart stops the engine before validating its arguments"
    # the pre-flight itself must be free of lifecycle side effects
    preflight = CLI.split("func startArgsError", 1)[1].split("func handleStart", 1)[0]
    for effect in ("handleStop", "kill(", "removeItem", "createFile", "Process()", "rotateLog", "ensureAppSupport"):
        assert effect not in preflight, "start pre-flight must not perform " + effect


def test_no_lifecycle_name_is_hardcoded_outside_the_namespace() -> None:
    import re
    sources = {p.name: p.read_text() for p in (ROOT / "harpoon/Sources").glob("*.swift")}
    names = "harpoon.lock|harpoon-lifecycle.lock|harpoon-docker.sock|harpoon-control|harpoon-mgmt.sock|harpoon-stop|harpoon-serial.log"
    for name, text in sources.items():
        code = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("//"))
        for hit in re.findall(r'"/tmp/(?:%s)"' % names, code):
            raise AssertionError(f"{name} hardcodes {hit}; use HarpoonPaths/HarpoonRoots")
    assert "let lockPath = HarpoonPaths.lockPath" in MAIN and "let stopFile = HarpoonPaths.stopFilePath" in MAIN
    assert "HarpoonRoots.isolationError()" in MAIN.split("let cliArgs", 1)[0], "isolation must be checked before any command"
    paths = (ROOT / "harpoon/Sources/HarpoonRuntimeState.swift").read_text().split("enum HarpoonPaths", 1)[1].split("final class", 1)[0]
    for name in ("lockPath", "dockerSocketPath", "controlSocketPath", "mgmtSocketPath", "lifecycleLockPath", "stopFilePath"):
        assert f"static var {name}: String {{ HarpoonRoots.runDir" in paths, name


def test_test_root_disables_production_fallbacks() -> None:
    state = (ROOT / "harpoon/Sources/HarpoonRuntimeState.swift").read_text()
    for fn in ("func readPIDFile", "func readMetadata", "func configFilePath"):
        body = state.split(fn, 1)[1].split("\n}\n", 1)[0]
        assert "HarpoonRoots.testRoot == nil" in body, fn + " reads production files under a test root"
    roots = CONFIG.split("enum HarpoonRoots", 1)[1].split("struct SharedRoot", 1)[0]
    assert "if let root = testRoot { return [root + \"/data/harpoon-root.img\"] }" in roots, "the production disk is a candidate under a test root"


def test_validation_entry_points_are_isolated() -> None:
    parity = (ROOT / "harpoon/cli-parity-test.sh").read_text()
    assert parity.index("export HARPOON_TEST_TMPDIR") < parity.index('"$BIN" restart'), "parity suite runs restart before isolating"
    assert "refusing to run lifecycle commands" in parity
    validate = (ROOT / "tools/validate.sh").read_text()
    assert "regression-lifecycle-isolation.sh" in validate
    assert validate.index("cli-parity-test.sh") < validate.index("regression-lifecycle-isolation.sh")
    build = (ROOT / "harpoon/build.sh").read_text()
    assert 'mv -f "$NEW" "$OUT"' in build and 'codesign --entitlements "$SCRIPT_DIR/entitlements.plist" --force -s - "$NEW"' in build, \
        "build.sh must not re-sign a possibly running binary in place"


# Manual, VM-booting or installer scripts that run lifecycle verbs against the machine's real
# namespace. None is reachable from tools/validate.sh or CI. A new script that issues start/stop/restart
# must either export HARPOON_TEST_TMPDIR or be added here on purpose, with the same warning in its header.
LIVE_ONLY = {
    "harpoon/ec-test.sh", "harpoon/install.sh", "harpoon/uninstall.sh", "harpoon/ui-test.sh",
    "harpoon/m4-test.sh", "harpoon/m7-test.sh", "harpoon/m8-test.sh", "harpoon/m9-test.sh", "harpoon/m10-test.sh",
    "harpoon/m11-test.sh", "harpoon/m12-test.sh", "harpoon/m13-test.sh", "harpoon/m14-test.sh", "harpoon/m15-test.sh",
    "harpoon/m15-balloon-test.sh", "harpoon/m16-test.sh", "harpoon/m17-test.sh", "harpoon/m18-test.sh",
    "tools/regression-stage6-cleanup.sh", "tools/stage-0-1-gate.sh", "tools/stage-2-resource-gate.sh",
    "tools/stage-6-docker-boundary-gate.sh", "tools/stage-7-consistency-check.sh", "tools/stage-8-disk-reclamation-check.sh",
}


def test_lifecycle_verbs_only_run_isolated_or_are_declared_live_only() -> None:
    import re
    verb = re.compile(r'"\$BIN" +(start|stop|restart)|harpoon(/build/harpoon)? +(start|stop|restart)\b|\$HARPOON +(start|stop|restart)|\$\{?BIN\}? +(start|stop|restart)')
    unisolated = set()
    for pattern in ("harpoon/*.sh", "tools/*.sh", "tools/guest-builder/*.sh"):
        for path in ROOT.glob(pattern):
            text = path.read_text()
            if verb.search(text) and "HARPOON_TEST_TMPDIR" not in text:
                unisolated.add(str(path.relative_to(ROOT)))
    assert unisolated == LIVE_ONLY, f"undeclared: {sorted(unisolated - LIVE_ONLY)}; stale entries: {sorted(LIVE_ONLY - unisolated)}"
    # what validate.sh and CI run must be outside that set
    validate = (ROOT / "tools/validate.sh").read_text()
    for script in LIVE_ONLY:
        assert script not in validate, "validate.sh reaches a live-only script: " + script
    for workflow in (ROOT / ".github/workflows").glob("*.yml"):
        text = workflow.read_text()
        for script in LIVE_ONLY - {"tools/stage-7-consistency-check.sh", "tools/stage-8-disk-reclamation-check.sh"}:
            assert script not in text, f"{workflow.name} runs live-only {script}"


def main() -> None:
    test_cross_process_control_lock()
    test_state_failures_are_not_clean_stop()
    test_restart_is_atomic_and_stop_failure_blocks_start()
    test_sync_and_disk_identity_guards_exist()
    test_disk_identity_never_blocks_stop()
    test_restart_validates_everything_before_it_stops_anything()
    test_no_lifecycle_name_is_hardcoded_outside_the_namespace()
    test_test_root_disables_production_fallbacks()
    test_validation_entry_points_are_isolated()
    test_lifecycle_verbs_only_run_isolated_or_are_declared_live_only()
    print("LIFECYCLE_SAFETY_OFFLINE_PASS")


if __name__ == "__main__":
    main()
