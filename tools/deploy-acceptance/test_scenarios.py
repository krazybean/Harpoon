"""Regression coverage for the three live-run defects of 2026-11-17 (evidence: ~/harpoon-acceptance-20261117-145101):
  1. reconcile was given --mgmt-socket, which it does not take: usage error rc 2, the recovery step silently skipped;
  2. a fault that never fired was reported as FAIL for the wrong reason (the proxy was rejected by the engine's socket-ownership guard);
  3. the cutover scenario started from a journal the previous scenario left unresolved, so the engine correctly refused (rc 6) and the harness
     reported "not killed" with no explanation.
Run: python3 tools/deploy-acceptance/test_scenarios.py   (no Docker, no Harpoon engine, no authorization)"""
import argparse, os, pathlib, re, sys, tempfile, unittest

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, str(HERE))
import real_docker_acceptance as h

ARGS_RS = HERE.parents[1] / "deploy-cli/src/args.rs"


def cli_flags(cmd):
    """The value flags `harpoon-deploy <cmd>` accepts, read from the CLI's own spec table."""
    m = re.search(r'command: "%s",\s*values: &\[(.*?)\]' % cmd, ARGS_RS.read_text(), re.S)
    return set(re.findall(r'"([\w-]+)"', m.group(1)))


class Scripted(h.Run):
    def __init__(self, tmp, status="unresolved cleanup: none\n", reconcile_rc=0):
        os.environ["HARPOON_LIVE_AUTH"] = "test-only"
        super().__init__(argparse.Namespace(simulate=False, cli=sys.executable, out=str(tmp), iterations=1, only=None, socket=None))
        self.live["forwarder-pid"] = "1"
        self.st, self.reconcile_rc, self.calls = status, reconcile_rc, []
        self.app = "app_X"
        self.journal = lambda: type("C", (), {"execute": lambda s, q: type("R", (), {"fetchone": lambda s: (1,)})()})()

    def status(self): return self.st
    def serving_ports(self): return [38000]
    def get(self, url, timeout=3): return 200, "ok"

    def cli(self, *a, live=True, **k):
        self.calls.append(a)
        if a[0] == "recover": return 0, "no deployment needed to be marked interrupted\n", "", 0.0
        if a[0] == "reconcile": return self.reconcile_rc, "usage error\n" if self.reconcile_rc else "", "", 0.0
        raise AssertionError(f"unexpected CLI call {a}")


class LiveFlags(unittest.TestCase):
    def test_every_live_flag_is_one_the_command_takes(self):
        r = Scripted(pathlib.Path(tempfile.mkdtemp()))
        for cmd in h.LIVE_FLAGS:
            taken = cli_flags(cmd) | {"authorize-live-docker-changes"}
            given = {x[2:].split("=")[0] for x in r.live_for(cmd) if x.startswith("--")}
            self.assertLessEqual(given, taken, f"{cmd} does not take {given - taken}")

    def test_reconcile_is_not_given_the_management_socket(self):                 # the live-run defect
        r = Scripted(pathlib.Path(tempfile.mkdtemp()))
        self.assertNotIn("--mgmt-socket", r.live_for("reconcile"))
        self.assertNotIn("--forwarder-pid", r.live_for("reconcile"))
        self.assertIn("--mgmt-socket", r.live_for("replace"))

    def test_a_socket_override_reaches_only_the_command_that_asked_for_it(self):
        r = Scripted(pathlib.Path(tempfile.mkdtemp()))
        self.assertIn("/x.sock", r.live_for("replace", **{"docker-socket": "/x.sock"}))
        self.assertNotIn("/x.sock", r.live_for("replace"))


class Recovery(unittest.TestCase):
    def test_a_recovery_command_that_fails_fails_the_scenario(self):             # previously only "unresolved" was checked
        r = Scripted(pathlib.Path(tempfile.mkdtemp()), reconcile_rc=2)
        r.after_kill("sigkill-x", 1.0, [38000], -9, [])
        self.assertFalse(r.results[-1]["ok"])
        self.assertEqual(r.results[-1]["reconcile_apply_rc"], 2)

    def test_a_clean_recovery_passes(self):
        r = Scripted(pathlib.Path(tempfile.mkdtemp()))
        r.after_kill("sigkill-x", 1.0, [38000], -9, [])
        self.assertTrue(r.results[-1]["ok"])


class Precondition(unittest.TestCase):
    def test_unresolved_work_stops_a_scenario_before_any_replacement_is_started(self):     # rc 6 in the live run
        r = Scripted(pathlib.Path(tempfile.mkdtemp()), status="unresolved cleanup (1):\n  image x\n")
        for fn in (r.kill_after_image_intent, r.kill_during_cutover, r.docker_api_failure, r.docker_name_conflict):
            fn()
        self.assertEqual([x["ok"] for x in r.results], [False] * 4)
        self.assertTrue(all("precondition" in x for x in r.results))
        self.assertEqual(r.calls, [], "no replacement and no recovery command was run")

    def test_an_unfinished_deployment_blocks(self):
        for state in ("running", "queued", "requested"):
            r = Scripted(pathlib.Path(tempfile.mkdtemp()), status=f"unresolved cleanup: none\n  dep_01ABC {state} app=app_X\n")
            self.assertFalse(r.ready_for_change("x"), state)
            self.assertIn("unfinished", r.results[-1]["blockers"][0])

    def test_an_unconfirmed_cutover_blocks(self):
        r = Scripted(pathlib.Path(tempfile.mkdtemp()), status="unresolved cleanup: none\n    cutover began and was never confirmed: completion is UNVERIFIED and the application MAY BE DOWN\n")
        self.assertFalse(r.ready_for_change("x"))

    # The 2026-11-17 17:36 run: `status` showed an interrupted deployment (from the SIGKILL scenario, already reconciled), a restored release
    # and a PAUSED application, with "unresolved cleanup: none". `replace` accepts that state; the harness must too.
    REAL = (SC := pathlib.Path(__file__).parent / "fixtures" / "status_interrupted_paused_clean.txt")

    def test_an_interrupted_deployment_and_a_recorded_pause_do_not_block(self):
        text = self.REAL.read_text()
        self.assertIn("interrupted", text); self.assertIn("PAUSED", text)
        self.assertEqual(h.Run.change_blockers(text), [])
        r = Scripted(pathlib.Path(tempfile.mkdtemp()), status=text)
        self.assertTrue(r.ready_for_change("x"))
        self.assertEqual(r.results, [])

    def test_the_blocker_is_named_when_there_is_one(self):
        r = Scripted(pathlib.Path(tempfile.mkdtemp()), status="unresolved cleanup (1):\n  image x\nPAUSED\n")
        self.assertFalse(r.ready_for_change("x"))
        self.assertEqual(r.results[-1]["blockers"], ["unresolved cleanup"])
        self.assertTrue(r.results[-1]["paused"])

    def test_a_clean_journal_is_ready(self):
        self.assertTrue(Scripted(pathlib.Path(tempfile.mkdtemp())).ready_for_change("x"))


class FaultInjection(unittest.TestCase):
    def test_only_the_preliminary_candidates_creation_is_targeted(self):
        t = h.Run.is_pre_create
        self.assertTrue(t("POST /containers/create?name=hp-dep_01ABC-pre HTTP/1.1"))
        self.assertFalse(t("POST /containers/create?name=hp-dep_01ABC HTTP/1.1"), "the final candidate is not the preliminary one")
        self.assertFalse(t("POST /containers/create?name=other-pre HTTP/1.1"))
        self.assertFalse(t("GET /containers/json HTTP/1.1"))
        self.assertFalse(t("POST /containers/abc/start HTTP/1.1"))

    def test_after_a_409_reconcile_may_only_propose_releasing_the_lease(self):
        ok = h.Run.only_lease_release
        self.assertTrue(ok(["would: release port lease 38025 of dep_X (ledger only; applied once that deployment's containers are settled)", "dry run: nothing was changed."]))
        self.assertFalse(ok(["would: REMOVE container hp-dep_X", "dry run: ..."]))
        self.assertFalse(ok(["REFUSE container hp-dep_X of dep_X: not ours", "would: release port lease 38025 of dep_X"][:1]))
        self.assertFalse(ok(["reconcile: nothing to do", "dry run: nothing was changed."]), "nothing at all means the lease was not left to release: the scenario did not reproduce")

    def test_a_fault_that_did_not_fire_never_passes(self):
        p = h.Run.fault_passed
        self.assertFalse(p(0, 7, True), "injection_fired=0 must fail even when everything else looks fine")
        self.assertFalse(p(1, 0, True))
        self.assertFalse(p(1, 7, False))
        self.assertTrue(p(1, 7, True))


if __name__ == "__main__":
    unittest.main()
