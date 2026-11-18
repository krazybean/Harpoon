#!/usr/bin/env python3
"""Real-Docker acceptance of deployment, replacement and recovery, against the existing Harpoon engine.

Everything runs against a DISPOSABLE application (a busybox web server) with its OWN deployment journal. It never deploys to,
stops or removes anything that existed before it started. Docker changes go through `harpoon-deploy`, which needs the live-change
authorization: the person running this exports it as HARPOON_LIVE_AUTH (the value after `=` in the flag the CLI prints). This file
never contains it.

  HARPOON_LIVE_AUTH='<phrase>' python3 real_docker_acceptance.py run --cli /path/to/harpoon-deploy
  python3 real_docker_acceptance.py run --simulate --cli ...        # orchestration check only: no Docker, no authorization
  python3 real_docker_acceptance.py teardown --out DIR              # remove ONLY resources labelled with the test app

Scenarios (one report.json + report.md in the evidence directory): setup, replace x N under journal polling, build failure,
health failure before cutover, Docker API failure at the preliminary candidate's creation (proxy), Docker name conflict at the final
candidate's creation (real 409), SIGKILL after the image intent, SIGKILL during
cutover (recover, restore, reconcile), repeated health checks through the host forwarder, ownership audit.
"""
import argparse, datetime, json, os, pathlib, re, shutil, signal, socket, sqlite3, subprocess, sys, threading, time, urllib.request

HOME = pathlib.Path.home()
SOCK, MGMT = "/tmp/harpoon-docker.sock", "/tmp/harpoon-mgmt.sock"
RES = "/Applications/Harpoon.app/Contents/Resources"
HARPOON_LOG = HOME / "Library/Application Support/Harpoon/harpoon.log"
RUNTIME_JSON = HOME / "Library/Application Support/Harpoon/runtime.json"
PROTECTED = {"Collectiv": "http://127.0.0.1:38000/", "juancastro.me": "http://127.0.0.1:38006/"}
PG = "collectiv-harpoon-test-db-1"
BASE_IMAGE = "busybox:1.36"   # Harpoon never pulls implicitly (deliberate): the base image must already be in the engine
PULL = f"docker --context harpoon pull {BASE_IMAGE}"
# The live flags each CLI command accepts (deploy-cli/src/args.rs is strict: an unknown flag is a usage error, rc 2). test_scenarios.py
# checks this table against args.rs, so a change to the CLI's flags fails a test instead of a live run.
LIVE_FLAGS = {
    "deploy": ("docker-socket", "mgmt-socket", "resources-dir", "forwarder-pid", "forwarder-command", "forward-address"),
    "replace": ("docker-socket", "mgmt-socket", "resources-dir", "forwarder-pid", "forwarder-command", "forward-address"),
    "restore": ("docker-socket", "mgmt-socket", "resources-dir", "forwarder-pid", "forwarder-command", "forward-address"),
    "reconcile": ("docker-socket", "resources-dir"),
}


def now(): return time.time()
def stamp(): return datetime.datetime.now(datetime.timezone.utc).strftime("%H:%M:%S.%f")[:-3]


class Run:
    def __init__(self, a):
        self.a, self.sim = a, a.simulate
        self.out = pathlib.Path(a.out or HOME / f"harpoon-acceptance-{datetime.datetime.now():%Y%m%d-%H%M%S}")
        self.out.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.rt, self.work = self.out / "runtime", self.out / "app"
        self.rt.mkdir(mode=0o700, exist_ok=True)
        self.results, self.app, self.auth = [], None, os.environ.get("HARPOON_LIVE_AUTH", "")
        if not self.sim and not self.auth:
            sys.exit("set HARPOON_LIVE_AUTH to the live-change authorization phrase (or use --simulate)")
        rj = json.loads(RUNTIME_JSON.read_text()) if RUNTIME_JSON.exists() else {}
        self.live = {"docker-socket": a.socket or SOCK, "mgmt-socket": MGMT, "resources-dir": RES,
                     "forwarder-pid": str(rj.get("pid", 0)), "forwarder-command": "harpoon"}
        if rj.get("forwardTarget", {}).get("address"):
            self.live["forward-address"] = rj["forwardTarget"]["address"]

    def live_for(self, cmd, **override):
        """The live flags this command accepts, plus the authorization. Never a flag the command does not take."""
        v = {**self.live, **override}
        out = [x for k in LIVE_FLAGS[cmd] if k in v for x in (f"--{k}", v[k])]
        return out + [f"--authorize-live-docker-changes={self.auth}"]

    # ---- plumbing
    def log(self, *m):
        line = f"{stamp()} " + " ".join(str(x) for x in m)
        print(line, flush=True)
        with open(self.out / "run.log", "a") as f: f.write(line + "\n")

    def cli(self, *args, live=True, timeout=900, **override):
        cmd = [self.a.cli, *args, "--runtime-dir", str(self.rt)]
        if self.sim: cmd.append("--simulate") if args[0] in ("deploy", "replace", "restore", "rollback") else None
        elif live: cmd += self.live_for(args[0], **override)
        t = now()
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        dt = now() - t
        safe = [c if not c.startswith("--authorize") else "--authorize-live-docker-changes=<redacted>" for c in cmd]
        with open(self.out / "cli.log", "a") as f: f.write(f"\n$ {' '.join(safe)}\n# rc={p.returncode} {dt:.2f}s\n{p.stdout}{p.stderr}")
        return p.returncode, p.stdout, p.stderr, dt

    def docker(self, *args):
        if self.sim: return ""
        return subprocess.run(["docker", "--context", "harpoon", *args], capture_output=True, text=True, timeout=120).stdout

    def docker_rc(self, *args):
        p = subprocess.run(["docker", "--context", "harpoon", *args], capture_output=True, text=True, timeout=120)
        return p.returncode, p.stdout

    def prerequisites(self):
        """Everything that must hold before the first deployment. Returns the failures (empty = ready). Changes nothing."""
        bad = []
        if not os.access(self.a.cli, os.X_OK): bad.append(f"the CLI is not executable: {self.a.cli}")
        if self.sim: return bad                                   # a simulated run needs no engine
        rc, _ = self.docker_rc("version", "--format", "{{.Server.Version}}")
        if rc != 0: bad.append("the Harpoon Docker engine does not answer (docker --context harpoon version)")
        else:
            rc, _ = self.docker_rc("image", "inspect", BASE_IMAGE)
            if rc != 0: bad.append(f"the base image {BASE_IMAGE} is not in the engine. Harpoon never pulls implicitly, so pull it first, yourself: {PULL}")
        return bad

    def get(self, url, timeout=3):
        try:
            with urllib.request.urlopen(url, timeout=timeout) as r: return r.status, r.read(200).decode(errors="replace").strip()
        except urllib.error.HTTPError as e: return e.code, ""
        except Exception as e: return 0, type(e).__name__

    def journal(self):
        p = self.rt / "deploy-journal/deployments.sqlite3"
        return sqlite3.connect(f"file:{p}?mode=ro", uri=True, timeout=5) if p.exists() else None

    def status(self): return self.cli("status", live=False)[1]

    def snapshot_docker(self):
        return {"containers": sorted(self.docker("ps", "-a", "--format", "{{.ID}} {{.Names}}").splitlines()),
                "images": sorted(self.docker("images", "-a", "--format", "{{.ID}} {{.Repository}}:{{.Tag}}").splitlines()),
                "volumes": sorted(self.docker("volume", "ls", "-q").splitlines()),
                "networks": sorted(self.docker("network", "ls", "--format", "{{.ID}} {{.Name}}").splitlines())}

    def protected_ok(self, tag):
        bad = []
        if self.sim: return True
        for n, u in PROTECTED.items():
            if self.get(u)[0] != 200: bad.append(n)
        h = self.docker("inspect", PG, "--format", "{{.State.Health.Status}} {{.RestartCount}}").split()
        if h[:1] != ["healthy"] or h[1:2] != ["0"]: bad.append(f"postgres {h}")
        self.log(f"protected-check[{tag}]", "OK" if not bad else f"BAD {bad}")
        if bad: self.record(f"protected-{tag}", False, bad=bad); raise SystemExit(f"protected application changed: {bad}")
        return True

    def record(self, name, ok, **kw):
        r = {"scenario": name, "ok": ok, "at": stamp(), **kw}
        self.results.append(r)
        (self.out / "report.json").write_text(json.dumps(self.results, indent=1, default=str))
        self.log("RESULT", name, "PASS" if ok else "FAIL", {k: v for k, v in kw.items() if k not in ("detail",)})

    # ---- the disposable application
    def write_app(self, version, mode="ok"):
        self.work.mkdir(exist_ok=True)
        (self.work / "www").mkdir(exist_ok=True)
        (self.work / "www/index.html").write_text(f"v{version}\n")
        (self.work / "www/health").write_text("ok\n")
        port = 3001 if mode == "wrongport" else 3000          # the health probe targets 3000: nothing answers there
        run = "false" if mode == "buildfail" else "true"
        (self.work / "Dockerfile").write_text(f"FROM busybox:1.36\nRUN {run}\nCOPY www /www\nCMD [\"httpd\",\"-f\",\"-p\",\"{port}\",\"-h\",\"/www\"]\n")

    def src(self): return ["--project-dir", str(self.work), "--container-port", "3000", "--health-path", "/health"]

    def serving_ports(self):
        if self.sim: return []
        out = self.docker("ps", "--filter", f"label=io.harpoon.app={self.app}", "--format", "{{.Ports}}")
        return sorted({int(m) for m in re.findall(r"0\.0\.0\.0:(\d+)->", out)})

    # ---- measurement threads
    def outage_monitor(self, stop, sink):
        up, since, last_up = True, None, now()
        while not stop.is_set():
            ports = self.serving_ports() if not self.sim else []
            ok = any(self.get(f"http://127.0.0.1:{p}/health", 1)[0] == 200 for p in ports)
            t = now()
            if ok: last_up = t
            if not ok and up and ports: up, since = False, last_up
            if ok and not up: sink.append({"down_s": round(t - since, 3), "from": since, "to": t}); up = True
            time.sleep(0.1)

    def poller(self, stop, stats):
        j = 0
        while not stop.is_set():
            c = self.journal()
            if c:
                try: c.execute("select count(*) from records").fetchone(); stats["reads"] += 1
                except Exception as e: stats["read_errors"] += 1; stats["last_error"] = str(e)
                c.close()
            j += 1
            if j % 10 == 0: self.cli("status", live=False); stats["status_calls"] += 1
            time.sleep(0.1)

    # ---- scenarios
    def setup(self):
        self.write_app(1)
        rc, out, err, dt = self.cli("deploy", *self.src())
        m = re.search(r"\(application (app_\w+)\)", out)
        ok = rc == 0 and m is not None and "result: succeeded" in out
        if m: self.app = m.group(1)
        self.record("setup-first-deployment", ok, rc=rc, secs=round(dt, 1), app=self.app, ports=self.serving_ports())
        if not ok: raise SystemExit("first deployment failed; see cli.log")

    def replacements(self, n):
        stop, sink, stats = threading.Event(), [], {"reads": 0, "read_errors": 0, "status_calls": 0}
        ths = [threading.Thread(target=self.outage_monitor, args=(stop, sink), daemon=True), threading.Thread(target=self.poller, args=(stop, stats), daemon=True)]
        [t.start() for t in ths]
        rows = []
        for i in range(2, n + 2):
            self.write_app(i)
            rc, out, err, dt = self.cli("replace", "--app", self.app, *self.src(), "--confirm-cutover")
            interr = re.search(r"measured interruption: (\d+) ms", out)
            ok = rc == 0 and "result: succeeded" in out
            rows.append({"n": i - 1, "ok": ok, "rc": rc, "secs": round(dt, 1), "engine_interruption_ms": int(interr.group(1)) if interr else None,
                         "failure": (re.search(r"failure at .*", out) or [None])[0], "body": self.get(f"http://127.0.0.1:{(self.serving_ports() or [0])[-1]}/")[1] if not self.sim else None})
            self.log("replacement", rows[-1])
        stop.set(); [t.join(2) for t in ths]
        st = self.status()
        self.record("replace-x%d-under-polling" % n, all(r["ok"] for r in rows) and stats["read_errors"] == 0 and " running " not in st,
                    successes=sum(r["ok"] for r in rows), attempts=n, polling=stats, outages=sink, rows=rows,
                    max_engine_interruption_ms=max([r["engine_interruption_ms"] or 0 for r in rows] or [0]), max_probe_outage_s=max([o["down_s"] for o in sink] or [0]))

    def expect_failed_and_serving(self, name, mode, *extra, expect=None):
        before = self.serving_ports()
        self.write_app(900 + len(self.results), mode)
        rc, out, err, dt = self.cli("replace", "--app", self.app, *self.src(), "--confirm-cutover", *extra)
        after = self.serving_ports()
        serving_ok = self.sim or (after == before and any(self.get(f"http://127.0.0.1:{p}/health", 3)[0] == 200 for p in after))
        fail = re.search(r"failure at (\w+): (.*)", out)
        ok = rc != 0 and serving_ok and (expect is None or (fail and fail.group(1) == expect))
        self.record(name, ok, rc=rc, secs=round(dt, 1), failed_at=fail.group(1) if fail else None, message=(fail.group(2)[:200] if fail else err[:200]),
                    serving_before=before, serving_after=after, status_tail=self.status().splitlines()[-6:])

    # ---- fault injection. A fault counts only if it demonstrably fired; a scenario whose fault did not fire fails.
    @staticmethod
    def change_blockers(status):
        """What `harpoon-deploy replace` itself refuses on (deploy-cli commands.rs: unfinished, unresolved, application_may_be_down), read from
        `status`. Deliberately NOT blockers: an `interrupted`/`failed` deployment (terminal), a recorded pause (it holds automatic triggers only;
        `replace` does not look at it) and retained images. Treating those as blockers rejected a clean journal in the 2026-11-17 17:36 run."""
        b = []
        if "unresolved cleanup: none" not in status: b.append("unresolved cleanup")
        b += [f"unfinished deployment: {m.group(0).strip()}" for m in re.finditer(r"^\s+dep_\w+ (?:requested|queued|running)\b.*$", status, re.M)]
        if "MAY BE DOWN" in status or "application may be down" in status: b.append("a cutover began and was never confirmed")
        return b

    def ready_for_change(self, name):
        """A new change is refused (rc 6) while the engine's own gate has blockers; that gate is correct and is not bypassed. A scenario starts
        only when `replace` would be accepted, and otherwise records exactly which blocker stopped it. The pause state is recorded, not judged."""
        st = self.status()
        bl = [] if self.sim else self.change_blockers(st)
        if not bl: return True
        self.record(name, False, precondition="the engine would refuse a new replacement; nothing was started", blockers=bl,
                    paused="PAUSED" in st, status_tail=st.strip().splitlines()[-12:])
        return False

    def settle(self):
        """The recovery path after an interrupted or failed run: recover (journal only), then reconcile as a dry run and with --apply."""
        out = {}
        rc, o, _, _ = self.cli("recover", live=False)
        m = re.search(r"marked interrupted: (dep_\w+)", o)
        out.update(recover_rc=rc, interrupted_deployment=m.group(1) if m else None)
        if self.sim: out.update(reconcile_plan=[], reconcile_dry_rc=0, reconcile_apply_rc=0, reconcile_tail=[]); return out
        rc_c, o_c, _, _ = self.cli("reconcile")
        rc_p, o_p, _, _ = self.cli("reconcile", "--apply")
        out.update(reconcile_plan=o_c.strip().splitlines()[-4:], reconcile_dry_rc=rc_c, reconcile_apply_rc=rc_p, reconcile_tail=o_p.strip().splitlines()[-3:])
        return out

    def serving_up(self): return self.sim or any(self.get(f"http://127.0.0.1:{p}/health", 3)[0] == 200 for p in self.serving_ports())

    @staticmethod
    def is_pre_create(line):
        """The request that creates the PRELIMINARY candidate container (before any cutover): POST /containers/create?name=hp-dep_..-pre"""
        m = re.match(r"POST /containers/create\?(?:\S*&)?name=(hp-dep_\w+-pre)(?:&\S*)? ", line)
        return m is not None

    @staticmethod
    def only_lease_release(plan):
        """After a definitive 409 the only thing reconcile may propose is releasing the candidate's port lease: no removal, no refusal."""
        acts = [l for l in plan if l.startswith("would: ")]
        return len(acts) == 1 and acts[0].startswith("would: release port lease ")

    @staticmethod
    def fault_passed(fired, rc, serving_up): return fired == 1 and rc != 0 and serving_up

    def docker_api_failure(self):
        """Docker answers 500 to the preliminary candidate's container creation. The engine's own guard (the Docker socket must be owned by
        the engine process) rejects any proxy at the health check, so the fault is placed where the proxy is still in the path: creation,
        which comes before the health check. The cutover-time creation is covered by `dockerconflict`."""
        name, proxy = "docker-api-failure-before-cutover", str(self.out / "docker-fault.sock")
        if not self.ready_for_change(name): return
        fired = {"n": 0}
        srv = socket.socket(socket.AF_UNIX); srv.bind(proxy); srv.listen(64)

        def pump(a, b):
            try:
                while (d := a.recv(65536)): b.sendall(d)
            except OSError: pass
            for x in (a, b):
                try: x.shutdown(socket.SHUT_RDWR)
                except OSError: pass

        def handle(c):
            line = c.recv(65536, socket.MSG_PEEK).split(b"\r\n", 1)[0].decode(errors="replace")
            if self.is_pre_create(line) and fired["n"] == 0:
                fired["n"] += 1
                c.recv(65536)
                body = b'{"message":"injected: container creation failed"}'
                c.sendall(b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (len(body), body)); c.close(); return
            u = socket.socket(socket.AF_UNIX); u.connect(SOCK)
            threading.Thread(target=pump, args=(c, u), daemon=True).start(); pump(u, c)

        def loop():
            while True:
                try: c, _ = srv.accept()
                except OSError: return
                threading.Thread(target=handle, args=(c,), daemon=True).start()
        threading.Thread(target=loop, daemon=True).start()
        before = self.serving_ports()
        self.write_app(950)
        rc, out, err, dt = self.cli("replace", "--app", self.app, *self.src(), "--confirm-cutover", **{"docker-socket": proxy})
        srv.close()
        fail = re.search(r"failure at (\w+): (.*)", out)
        up = self.serving_up()
        st = self.settle()
        self.record(name, self.fault_passed(fired["n"], rc, up) and st["reconcile_apply_rc"] == 0 and "unresolved cleanup: none" in self.status(),
                    injection_fired=fired["n"], rc=rc, secs=round(dt, 1), failed_at=fail.group(1) if fail else None, message=(fail.group(2)[:200] if fail else (out + err)[-300:]),
                    serving_up_after=up, serving_before=before, serving_after=self.serving_ports(), **st)

    def docker_name_conflict(self):
        """A real Docker failure at the FINAL candidate's creation, which happens after the current release was stopped: a disposable
        container (no Harpoon labels) is created under the exact name the engine will use for the candidate, so Docker itself answers
        409 Conflict. The engine must restore the previous release and must not touch a container it does not own."""
        name = "docker-api-failure-at-cutover-name-conflict"
        if not self.ready_for_change(name): return
        before, t0 = self.serving_ports(), now()
        self.write_app(960)
        cmd = [self.a.cli, "replace", "--app", self.app, *self.src(), "--confirm-cutover", "--runtime-dir", str(self.rt), *self.live_for("replace")]
        res = {"conflict_id": None, "conflict_name": None}

        def plant():
            t_end = now() + 120
            while now() < t_end and not res["conflict_id"]:
                c = self.journal()
                if c:
                    try:
                        r = c.execute("select body from records where kind='snapshot' and seq > ? order by seq desc limit 1", (self.baseline_seq,)).fetchone()
                        f = r[0].split("\t") if r else []
                        if len(f) > 6 and f[6] == "running":
                            nm = f"hp-dep_{f[3].removeprefix('dep_')}"
                            rc, o = self.docker_rc("create", "--name", nm, "--label", f"io.harpoon.acceptance={self.out.name}", BASE_IMAGE, "true")
                            if rc == 0: res.update(conflict_id=o.strip(), conflict_name=nm)
                    except Exception: pass
                    c.close()
                time.sleep(0.05)
        th = threading.Thread(target=plant, daemon=True); th.start()
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
        th.join(5)
        (self.out / "dockerconflict.out").write_text(p.stdout + p.stderr)
        fail = re.search(r"failure at (\w+): (.*)", p.stdout)
        up = self.serving_up()
        untouched = None
        if res["conflict_id"]:
            rc, o = self.docker_rc("inspect", res["conflict_id"])
            (self.out / "foreign-container-inspect.json").write_text(o)               # evidence first; nothing is removed before this exists
            try: info = json.loads(o)[0]; untouched = rc == 0 and info["Name"] == "/" + res["conflict_name"] and info["Config"]["Labels"].get("io.harpoon.acceptance") == self.out.name and info["State"]["Status"] != "removing"
            except Exception: untouched = False
        st = self.settle()                                                              # reconcile runs WITH the foreign container still present
        still = res["conflict_id"] and self.docker_rc("inspect", res["conflict_id"])[0] == 0
        if untouched and still: self.docker_rc("rm", res["conflict_id"])                # exact id, verified name and label: ours
        plan_ok = self.only_lease_release(st.get("reconcile_plan", []))
        ok = bool(res["conflict_id"]) and p.returncode != 0 and up and untouched is True and bool(still) and st["reconcile_apply_rc"] == 0 and plan_ok \
            and "unresolved cleanup: none" in self.status()
        self.record(name, ok, conflict_created=bool(res["conflict_id"]), foreign_present_through_reconcile=bool(still), reconcile_plan_only_lease_release=plan_ok, conflict_name=res["conflict_name"], conflict_untouched_by_engine=untouched, rc=p.returncode,
                    failed_at=fail.group(1) if fail else None, message=(fail.group(2)[:240] if fail else (p.stdout + p.stderr)[-300:]), serving_up_after=up,
                    serving_before=before, serving_after=self.serving_ports(), secs=round(now() - t0, 1), **st)

    def kill_when(self, name, predicate):
        """Start a replacement, SIGKILL the CLI the moment the journal satisfies `predicate`. Returns (killed_at, serving_before, rc, output)."""
        self.write_app(970 + len(self.results))
        cmd = [self.a.cli, "replace", "--app", self.app, *self.src(), "--confirm-cutover", "--runtime-dir", str(self.rt)]
        if self.sim: cmd.append("--simulate")
        else: cmd += self.live_for("replace")
        before = self.serving_ports()
        outf = self.out / f"{name}.out"
        with open(outf, "w") as f: p = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT)
        killed = None; t_end = now() + 600
        while p.poll() is None and now() < t_end:
            c = self.journal()
            if c:
                try:
                    if predicate(c): p.send_signal(signal.SIGKILL); killed = now(); break
                except Exception: pass
                c.close()
            time.sleep(0.01)
        p.wait()
        tail = outf.read_text(errors="replace").strip().splitlines()[-6:]
        self.log(name, "killed" if killed else "NOT killed (the run finished first)", "rc", p.returncode, tail[-1:] if not killed else "")
        return killed, before, p.returncode, tail

    def kill_after_image_intent(self):
        name = "sigkill-after-image-intent"
        if not self.ready_for_change(name): return
        def pred(c):
            r = c.execute("select body from records where kind='ledger' and body like '%\tintent\timage\t%' order by seq desc limit 1").fetchone()
            if not r: return False
            tag = r[0].split("\t")[3]
            return c.execute("select count(*) from records where kind='ledger' and body like ? and body like '%\treceipt\timage\t%'", (f"%{tag}%",)).fetchone()[0] == 0 \
                and c.execute("select count(*) from records where kind='ledger' and body like ?", (f"%\tintent\timage\t{tag}%",)).fetchone()[0] == 1 and c.execute("select count(*) from records").fetchone()[0] > self.baseline_seq
        self.after_kill(name, *self.kill_when("kill-after-image-intent", pred))

    def kill_during_cutover(self, early=False):
        """Two kill points inside the cutover. `early`: the instant it begins, while the previous release's `stop` (10 s of grace for an
        application that ignores SIGTERM) is still running in Docker, so the previous container is still `running` when recovery starts.
        Otherwise: after the previous release stopped, when the engine records the intent to create the candidate's container."""
        name = "sigkill-at-cutover-begin" if early else "sigkill-during-cutover"
        if not self.ready_for_change(name): return
        def pred(c):
            r = c.execute("select body from records where kind='snapshot' order by seq desc limit 1").fetchone()
            if not (bool(r) and "\tbegun\t" in r[0] and c.execute("select count(*) from records").fetchone()[0] > self.baseline_seq): return False
            if early: return True
            return c.execute("select count(*) from records where kind='ledger' and seq > ? and body like '%\tintent\tcontainer\thp-dep_%' and body not like '%-pre\t%'", (self.baseline_seq,)).fetchone()[0] > 0
        self.after_kill(name, *self.kill_when(name, pred), restore=True)

    def after_kill(self, name, killed, before, rc, tail, restore=False):
        t0 = now()
        rc_r, out_r, _, _ = self.cli("recover", live=False)
        dep = re.search(r"marked interrupted: (dep_\w+)", out_r)
        steps = {"killed": bool(killed), "interrupted_deployment": dep.group(1) if dep else None, "replace_rc": rc, "replace_tail": tail}
        if restore and dep:
            rc_d, out_d, _, _ = self.cli("restore", "--deployment", dep.group(1))                    # dry run
            rc_a, out_a, _, dt_a = self.cli("restore", "--deployment", dep.group(1), "--apply")
            steps.update(restore_dry_rc=rc_d, restore_rc=rc_a, restore_secs=round(dt_a, 1), restore_tail=out_a.strip().splitlines()[-3:])
        if self.sim: rc_c = rc_p = 0; out_c = out_p = ""                                             # a simulated runtime has nothing to reconcile
        else:
            rc_c, out_c, _, _ = self.cli("reconcile")                                                # dry run
            rc_p, out_p, _, _ = self.cli("reconcile", "--apply")
        st = self.status()
        up = self.serving_up()
        unresolved = re.findall(r"unresolved cleanup \((\d+)\)", st)
        clean = rc_c == 0 and rc_p == 0 and (self.sim or not unresolved)                              # a failing recovery command is a failure, not a skipped step
        self.record(name, (bool(killed) or self.sim) and up and clean and (not restore or not dep or steps["restore_rc"] == 0),
                    recovery_secs=round(now() - t0, 1), reconcile_plan=out_c.strip().splitlines()[-4:], reconcile_dry_rc=rc_c,
                    reconcile_apply_rc=rc_p, reconcile_tail=out_p.strip().splitlines()[-3:], serving_up=up, unresolved=unresolved or 0,
                    serving_before=before, serving_after=self.serving_ports(), **steps)
        self.baseline_seq = (lambda c: c.execute("select max(seq) from records").fetchone()[0])(self.journal())

    def forwarder_probes(self, n=300):
        log_before = HARPOON_LOG.read_text(errors="replace").count("HOST_FORWARD_CONNECT_FAILED") if HARPOON_LOG.exists() else 0
        ports, fails, t0 = self.serving_ports() or [0], [], now()
        for i in range(n):
            s = self.get(f"http://127.0.0.1:{ports[-1]}/health", 2)[0]
            if s != 200: fails.append({"i": i, "t": stamp(), "status": s})
            time.sleep(0.1)
        log_after = HARPOON_LOG.read_text(errors="replace").count("HOST_FORWARD_CONNECT_FAILED") if HARPOON_LOG.exists() else 0
        self.record("repeated-health-checks-through-forwarder", not fails, probes=n, failures=len(fails), first_failures=fails[:5], secs=round(now() - t0, 1),
                    forwarder_connect_failed_log_delta=log_after - log_before)

    def audit(self, before):
        after = self.snapshot_docker()
        added = {k: sorted(set(after[k]) - set(before[k])) for k in after}
        removed = {k: sorted(set(before[k]) - set(after[k])) for k in after}
        foreign = [x for k in added for x in added[k] if self.app and self.app.lower() not in x.lower() and not re.search(r"hp-dep_|busybox|harpoon/app_", x)]
        db = self.journal(); integ = db.execute("pragma integrity_check").fetchone()[0] if db else "n/a"
        active = self.active_deployments(self.status())
        self.record("ownership-audit", not removed["containers"] + removed["images"] + removed["volumes"] + removed["networks"] and not foreign and integ == "ok" and not active,
                    added=added, removed=removed, foreign_additions=foreign, journal_integrity=integ, active_deployments=len(active), active=active)

    @staticmethod
    def active_deployments(status):
        """Deployment rows still `running`. Not a substring test: `status` also says "no running release is confirmed" in prose, which
        made a failed restoration look like an active deployment in the 2026-11-17 17:48 audit."""
        return re.findall(r"^\s+(dep_\w+) running\b", status, re.M)

    def main(self):
        missing = self.prerequisites()
        if missing:
            for m in missing: self.log("PREREQUISITE FAILED:", m)
            self.record("prerequisites", False, missing=missing)
            (self.out / "report.md").write_text("# Real-Docker acceptance\n\nNOT STARTED: prerequisites failed. Nothing was deployed or changed.\n\n" + "\n".join(f"- {m}" for m in missing) + "\n")
            return 2
        self.record("prerequisites", True)
        before = self.snapshot_docker()
        (self.out / "docker-before.json").write_text(json.dumps(before, indent=1))
        self.protected_ok("start")
        self.setup()
        self.baseline_seq = self.journal().execute("select max(seq) from records").fetchone()[0]
        steps = [("replace", lambda: self.replacements(self.a.iterations)),
                 ("buildfail", lambda: self.expect_failed_and_serving("build-failure-before-cutover", "buildfail", expect="build")),
                 ("healthfail", lambda: self.expect_failed_and_serving("health-failure-before-cutover", "wrongport", "--health-timeout-secs", "20", expect="preliminary_check")),
                 ("dockerapi", self.docker_api_failure), ("dockerconflict", self.docker_name_conflict), ("killintent", self.kill_after_image_intent), ("killcutover", self.kill_during_cutover), ("killearly", lambda: self.kill_during_cutover(early=True)),
                 ("forwarder", self.forwarder_probes)]
        for key, fn in steps:
            if self.a.only and key not in self.a.only: continue
            if self.sim and key in ("buildfail", "healthfail", "dockerapi", "dockerconflict", "forwarder"): self.record(key, True, skipped="needs real Docker"); continue
            self.log("== scenario", key)
            try: fn()
            except SystemExit: raise
            except Exception as e: self.record(key, False, error=f"{type(e).__name__}: {e}")
            self.protected_ok(key)
        self.audit(before)
        ok = all(r["ok"] for r in self.results)
        (self.out / "report.md").write_text("# Real-Docker acceptance\n\n" + "\n".join(f"- {'PASS' if r['ok'] else 'FAIL'}  {r['scenario']}" for r in self.results) + f"\n\nOVERALL: {'PASS' if ok else 'FAIL'}\n")
        self.log("OVERALL", "PASS" if ok else "FAIL", "evidence:", self.out)
        return 0 if ok else 1


def teardown(a):
    """Remove only what carries the test application's id (read from the evidence report), by exact id, after printing it."""
    rep = json.loads((pathlib.Path(a.out) / "report.json").read_text())
    app = next(r["app"] for r in rep if r.get("app"))
    d = lambda *x: subprocess.run(["docker", "--context", "harpoon", *x], capture_output=True, text=True).stdout
    cs = [l.split() for l in d("ps", "-aq", "--filter", f"label=io.harpoon.app={app}", "--format", "{{.ID}} {{.Names}} {{.Label \"io.harpoon.app\"}}").splitlines()]
    imgs = [l.split() for l in d("images", "-a", "--format", "{{.ID}} {{.Repository}}").splitlines() if l.split()[1].startswith(f"harpoon/{app.lower()}")]
    print("containers to remove:", cs, "\nimages to remove:", imgs)
    if not a.yes: print("dry run; add --yes"); return 0
    for c in cs:
        assert c[2] == app and c[1].startswith("hp-dep_"), c
        print(d("rm", "-f", c[0]).strip())
    for i in imgs: print(d("rmi", i[0]).strip())
    return 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser(); sp = ap.add_subparsers(dest="cmd", required=True)
    r = sp.add_parser("run"); r.add_argument("--cli", required=True); r.add_argument("--out"); r.add_argument("--simulate", action="store_true")
    r.add_argument("--iterations", type=int, default=20); r.add_argument("--only", nargs="*"); r.add_argument("--socket")
    t = sp.add_parser("teardown"); t.add_argument("--out", required=True); t.add_argument("--yes", action="store_true")
    a = ap.parse_args()
    sys.exit(Run(a).main() if a.cmd == "run" else teardown(a))
