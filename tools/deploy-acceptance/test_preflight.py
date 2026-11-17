"""Regression coverage for the harness's prerequisite check: a missing base image stops the run before any deployment.
Run: python3 tools/deploy-acceptance/test_preflight.py   (no Docker, no Harpoon engine, no authorization)"""
import argparse, os, pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import real_docker_acceptance as h


class Stub(h.Run):
    """A run whose Docker answers are scripted and whose CLI must never be reached when prerequisites fail."""
    def __init__(self, tmp, image_present, engine_up=True):
        os.environ["HARPOON_LIVE_AUTH"] = "test-only"
        super().__init__(argparse.Namespace(simulate=False, cli=sys.executable, out=str(tmp), iterations=1, only=None, socket=None))
        self.image_present, self.engine_up, self.calls, self.docker_calls = image_present, engine_up, [], []

    def docker_rc(self, *args):
        self.docker_calls.append(args)
        if args[0] == "version": return (0 if self.engine_up else 1), "28.3.3"
        if args[:2] == ("image", "inspect"): return (0 if self.image_present else 1), ""
        raise AssertionError(f"a prerequisite check must only read: {args}")

    def cli(self, *a, **k): self.calls.append(a); raise AssertionError("the deployment sequence began")
    def snapshot_docker(self): raise AssertionError("the deployment sequence began")


class Preflight(unittest.TestCase):
    def setUp(self): self.tmp = pathlib.Path(tempfile.mkdtemp())

    def test_a_missing_base_image_is_a_prerequisite_failure_with_the_exact_pull_command(self):
        r = Stub(self.tmp, image_present=False)
        missing = r.prerequisites()
        self.assertEqual(len(missing), 1)
        self.assertIn("docker --context harpoon pull busybox:1.36", missing[0])
        self.assertIn("never pulls implicitly", missing[0])

    def test_nothing_is_deployed_and_nothing_is_pulled_when_the_image_is_missing(self):
        r = Stub(self.tmp, image_present=False)
        self.assertEqual(r.main(), 2)
        self.assertEqual(r.calls, [], "no CLI call (deploy, replace, status, ...) was made")
        self.assertTrue(all(c[0] in ("version", "image") for c in r.docker_calls), r.docker_calls)
        self.assertFalse(any("pull" in c for c in r.docker_calls), "the harness must not pull for the user")
        report = (r.out / "report.md").read_text()
        self.assertIn("NOT STARTED", report)
        self.assertIn("docker --context harpoon pull busybox:1.36", report)
        self.assertFalse((r.out / "docker-before.json").exists(), "the protected-resource snapshot is taken only after prerequisites pass")

    def test_an_engine_that_does_not_answer_is_reported_and_stops_the_run(self):
        r = Stub(self.tmp, image_present=True, engine_up=False)
        self.assertEqual(r.main(), 2)
        self.assertIn("does not answer", (r.out / "report.md").read_text())
        self.assertEqual(r.calls, [])

    def test_all_prerequisites_present_lets_the_sequence_begin(self):
        r = Stub(self.tmp, image_present=True)
        self.assertEqual(r.prerequisites(), [])
        with self.assertRaises(AssertionError) as e: r.main()          # the stub refuses the first real step, proving it was reached
        self.assertIn("deployment sequence began", str(e.exception))

    def test_a_simulated_run_needs_no_engine_or_image(self):
        r = Stub(self.tmp, image_present=False)
        r.sim = True
        self.assertEqual(r.prerequisites(), [])


if __name__ == "__main__":
    unittest.main()
