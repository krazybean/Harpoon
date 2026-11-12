#!/usr/bin/env python3
"""Behavioural tests for tools/vm-test-guard.py. No VM: engines and VZ helpers are small fake processes whose command lines carry
'hpt-fake-engine' / 'hpt-fake-helper' (the guard's test-only match hooks). Usage: regression-vm-test-guard.py [scenario ...]
VMGUARD_BIN selects the guard under test (the driver points it at mutated copies)."""
import importlib.util, json, os, shutil, signal, subprocess, sys, tempfile, threading, time
sys.dont_write_bytecode = True  # importing the guard must not leave a __pycache__ in the tree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUARD = os.environ.get("VMGUARD_BIN", os.path.join(ROOT, "tools", "vm-test-guard.py"))
W = tempfile.mkdtemp(prefix="hpt-guard-test.", dir="/tmp")  # short: private namespaces live under /tmp too
TAG = os.path.basename(W)
ENV = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", VMGUARD_TEST="1", VMGUARD_ENGINE_MATCH="hpt-fake-engine", VMGUARD_HELPER_MATCH="hpt-fake-helper",
           VMGUARD_LEDGER_DIR=W + "/ledger", VMGUARD_METRICS_FILE=W + "/metrics.json", VMGUARD_DIAG_CAP="8192")
FAST = ["--shutdown-grace", "1.5", "--cleanup-grace", "1.5", "--sample-interval", "0.2", "--admission-window", "0.3"]
os.environ.update({k: v for k, v in ENV.items() if k.startswith("VMGUARD_")})
_spec = importlib.util.spec_from_file_location("vmguard", GUARD); G = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(G)
failures = []

ENGINE = W + "/hpt-fake-engine.py"
open(ENGINE, "w").write('''import fcntl, json, os, signal, sys, time
rj, mode, rd = sys.argv[1:4]
os.setsid()   # a detached daemon, like `harpoon start` leaves behind
if mode in ("ignore", "stopped"): signal.signal(signal.SIGTERM, signal.SIG_IGN)
if mode == "witness":   # records every signal it is ever sent; survives them all so the test can read the record
    def seen(n, f): open(rd + "/witness.log", "a").write(f"signal {n}\\n")
    for n in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP, signal.SIGUSR1, signal.SIGUSR2, signal.SIGQUIT): signal.signal(n, seen)
if mode in ("lock", "witness"): lf = open(rd + "/harpoon.lock", "w"); fcntl.flock(lf, fcntl.LOCK_EX)   # the real engine holds this for its life
json.dump({"pid": os.getpid()}, open(rj + ".tmp", "w")); os.replace(rj + ".tmp", rj)
if mode == "stopped": os.kill(os.getpid(), signal.SIGSTOP)   # a blocked main queue: no signal handler can ever run
while True:
    if mode == "graceful" and os.path.exists(rd + "/harpoon-stop"): sys.exit(0)
    time.sleep(0.05)
''')
os.makedirs(W + "/actions-runner", exist_ok=True)
shutil.copy(ENGINE, W + "/actions-runner/hpt-fake-engine.py")
HELPER = W + "/hpt-fake-helper.py"
open(HELPER, "w").write("import os, time\nos.setsid()\ntime.sleep(300)\n")

def metrics(disk=100, pressure=1, swap_used=0, swap_total=10 * 2**30, swap_out=0):
    json.dump({"disk_free": disk * 2**30, "pressure": pressure, "swap_used": swap_used, "swap_total": swap_total, "swap_out": swap_out}, open(W + "/metrics.json", "w"))

def guard(args, cmd, timeout=60, env=None):
    t = time.time()
    r = subprocess.run([sys.executable, GUARD] + args + ["--"] + cmd, capture_output=True, text=True, env=env or ENV, timeout=timeout)
    return r.returncode, r.stderr, time.time() - t

def sh(script): return ["sh", "-c", script]
def pid_alive(pid):
    try: os.kill(pid, 0)
    except OSError: return False
    out = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return bool(out) and not out.startswith("Z")
def wait_until(fn, seconds=10):
    end = time.time() + seconds
    while time.time() < end:
        if fn(): return True
        time.sleep(0.1)
    return fn()
WAIT_RJ = 'i=0; while [ ! -s "{rj}" ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done; '
def engine_cmd(mode, copy_to):  # starts a detached fake engine in this run's namespace, records its pid, waits for it to register
    return (f'python3 {ENGINE} "$HARPOON_TEST_TMPDIR/runtime.json" {mode} "$HARPOON_TEST_TMPDIR" </dev/null >/dev/null 2>&1 & '
            + WAIT_RJ.format(rj="$HARPOON_TEST_TMPDIR/runtime.json") + f'cp "$HARPOON_TEST_TMPDIR/runtime.json" {copy_to}; ')
def pid_of(path): return json.load(open(path))["pid"]
def spawn_engine(rj, mode, rd):
    p = subprocess.Popen([sys.executable, ENGINE, rj, mode, rd], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: os.path.exists(rj)); return pid_of(rj), p
def check(ok, what):
    print(("  ok   " if ok else "  FAIL ") + what)
    if not ok: failures.append(what)
def prod_flags(rd, extra=()):
    return ["--namespace", "production", "--run-dir", rd, "--runtime-json", rd + "/runtime.json", "--deadline", "20"] + FAST + list(extra)
def ledger_files(): return [f for f in os.listdir(W + "/ledger") if f.endswith(".json")] if os.path.isdir(W + "/ledger") else []

unrelated = None  # an engine-looking process that no ledger, marker or runtime.json references: must survive everything

def s_passthrough():
    metrics(); check(guard(FAST, sh("exit 0"))[0] == 0, "exit 0 passes through"); check(guard(FAST, sh("exit 3"))[0] == 3, "exit 3 passes through")

def s_deadline():
    metrics(); rc, err, el = guard(["--deadline", "2"] + FAST, sh(f"echo $$ > {W}/dl.pid; exec sleep 6"))
    check(rc == 124, f"deadline expiry returns 124 (got {rc})"); check(el < 5.5, f"and stops the command at the deadline, not when it ends ({el:.1f}s)")
    check(not pid_alive(int(open(W + "/dl.pid").read())), "the command is dead")

def s_engine_cleanup():
    metrics()
    for mode in ("normal", "ignore", "stopped", "graceful"):
        pf = f"{W}/e-{mode}.json"
        rc, err, el = guard(["--deadline", "20"] + FAST, sh(engine_cmd(mode, pf) + "exit 0"))
        check(rc == 0 and wait_until(lambda: not pid_alive(pid_of(pf)), 2), f"owned '{mode}' engine is gone and the run still succeeds (rc {rc}, {el:.1f}s)")

def s_failed_cleanup():
    metrics(); env = dict(ENV, VMGUARD_TEST_SIGKILL_NOOP="1"); pf = W + "/fc.json"
    rc, err, _ = guard(["--deadline", "20", "--diag", W + "/fc.diag"] + FAST, sh(engine_cmd("ignore", pf) + "exit 0"), env=env)
    check(rc == 71, f"a cleanup that cannot verify exit is never success (rc {rc})"); check("survived cleanup" in err, "and says which process survived")
    check(os.path.exists(W + "/fc.diag"), "and leaves bounded diagnostics")
    os.kill(pid_of(pf), signal.SIGKILL)

def s_own_namespace():  # a test that sets its own HARPOON_TEST_TMPDIR still has its detached engine tracked and cleaned
    metrics()
    def cmd(tag):
        ns = f"{W}/own-{tag}"
        return sh(f'NS={ns}; mkdir -p $NS; HARPOON_TEST_TMPDIR=$NS python3 {ENGINE} $NS/runtime.json normal $NS </dev/null >/dev/null 2>&1 & '
                  + WAIT_RJ.format(rj=f"{ns}/runtime.json") + f"cp {ns}/runtime.json {W}/own-{tag}.json; exit 0")
    rc, err, _ = guard(["--deadline", "20"] + FAST, cmd("private"))
    check(rc == 0 and wait_until(lambda: not pid_alive(pid_of(W + "/own-private.json")), 3), f"private mode: engine in a test-chosen namespace is cleaned (rc {rc})")
    check("owns engine" in err, "and it was registered as owned")
    rd = W + "/own-prod-run"; os.makedirs(rd, exist_ok=True)
    rc, err, _ = guard(prod_flags(rd), cmd("prod"))
    check(rc == 0 and wait_until(lambda: not pid_alive(pid_of(W + "/own-prod.json")), 3), f"production mode: same (rc {rc})")

def s_production_exclusive():
    metrics()
    for label, mode in (("a held engine lock", "lock"), ("a live engine record", "normal")):
        rd = f"{W}/px-{mode}"; os.makedirs(rd, exist_ok=True)
        pid, e = spawn_engine(rd + "/runtime.json", mode, rd)
        rc, err, _ = guard(prod_flags(rd), sh(f"touch {W}/px.ran"))
        check(rc == 75 and not os.path.exists(W + "/px.ran") and pid_alive(pid), f"{label}: a pre-existing production engine is rejected, never run beside or touched (rc {rc})")
        if mode == "lock":
            rc, _, _ = guard(prod_flags(rd, ["--adopt-existing"]), sh("exit 0"))
            check(rc == 0 and pid_alive(pid), "--adopt-existing runs beside it and still leaves it alone")
        e.kill()
    rd = W + "/px-conc"; os.makedirs(rd, exist_ok=True); res = {}
    t = threading.Thread(target=lambda: res.__setitem__("first", guard(prod_flags(rd), sh("sleep 3.5; exit 0"))))
    t.start(); time.sleep(1.2)
    res["second"] = guard(prod_flags(rd), sh(f"touch {W}/px2.ran"))
    t.join()
    check(res["second"][0] == 75 and not os.path.exists(W + "/px2.ran") and "another test run" in res["second"][1], "a concurrent run in the production namespace is rejected")
    check(res["first"][0] == 0, "and the run that owns it is unaffected")

def s_production_ownership():
    metrics(); rd = W + "/prod"; os.makedirs(rd, exist_ok=True); rj = rd + "/runtime.json"
    a_pid, a = spawn_engine(rj, "normal", rd)   # a healthy engine that predates the run (adopted, so unowned)
    flags = prod_flags(rd, ["--adopt-existing"])
    rc, _, _ = guard(flags, sh("sleep 0.5; exit 0"))
    check(rc == 0 and pid_alive(a_pid), "an adopted pre-existing engine is never owned or touched")
    time.sleep(1.2)
    rc, _, _ = guard(flags, sh(f'python3 {ENGINE} {rj} normal {rd} </dev/null >/dev/null 2>&1 & sleep 1.5; cp {rj} {W}/b.json; exit 0'))
    b_pid = pid_of(W + "/b.json")
    check(b_pid != a_pid and not pid_alive(b_pid), "an engine the run started is stopped")
    check(pid_alive(a_pid), "while the adopted engine, still alive, is left running")
    a.kill()

def s_adopt_existing():  # --adopt-existing: the pre-existing engine is never signalled, stopped (stop file) or swept, however the run ends
    metrics(); rd = W + "/adopt"; os.makedirs(rd, exist_ok=True); rj = rd + "/runtime.json"
    pid, e = spawn_engine(rj, "witness", rd)
    before = open(rj).read()
    def untouched(label):
        time.sleep(0.3)
        ok = pid_alive(pid) and not os.path.exists(rd + "/witness.log") and not os.path.exists(rd + "/harpoon-stop") and open(rj).read() == before
        check(ok, f"{label}: engine alive, no signal received, no stop file, runtime record unchanged")
    flags = prod_flags(rd, ["--adopt-existing"])
    check(guard(flags, sh("exit 0"))[0] == 0, "adopted run succeeds beside the engine"); untouched("after a successful run")
    check(guard(flags, sh("exit 1"))[0] == 1, "a failing run keeps its own status"); untouched("after a failing run")
    rc, _, _ = guard(["--namespace", "production", "--run-dir", rd, "--runtime-json", rj, "--deadline", "1.5", "--adopt-existing"] + FAST, sh("exec sleep 6"))
    check(rc == 124, f"a run that hits its deadline is cleaned up (rc {rc})"); untouched("after a deadline expiry")
    g = subprocess.Popen([sys.executable, GUARD, "--namespace", "production", "--run-dir", rd, "--runtime-json", rj, "--deadline", "100", "--adopt-existing"] + FAST + ["--", "sh", "-c", "exec sleep 100"],
                         env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: ledger_files(), 5); time.sleep(0.8); g.send_signal(signal.SIGTERM); g.wait(timeout=20)
    untouched("after the guard was interrupted")
    g = subprocess.Popen([sys.executable, GUARD, "--namespace", "production", "--run-dir", rd, "--runtime-json", rj, "--deadline", "100", "--adopt-existing"] + FAST + ["--", "sh", "-c", "exec sleep 101"],
                         env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: ledger_files(), 5); time.sleep(0.8); g.kill(); g.wait()
    check(wait_until(lambda: not ledger_files(), 12), "after SIGKILL of the guard the reaper cleaned up and removed its ledger")
    untouched("after the reaper acted")
    subprocess.run([sys.executable, GUARD, "--sweep"], capture_output=True, text=True, env=ENV)
    guard(flags, sh("exit 0"))   # every guarded run also sweeps first
    untouched("after explicit and implicit sweeps")
    e.kill()

def s_pid_identity():  # unit level: signals follow (pid, microsecond start), never a bare pid
    p = subprocess.Popen(["sleep", "30"]); time.sleep(0.2)
    good = G.ident(p.pid)
    check("." in good["start"] and len(good["start"].split(".")[1]) == 6, f"identity carries a microsecond start time ({good['start']})")
    check(not G.alive({"pid": p.pid, "start": "1.000000"}) and not G.safe_kill({"pid": p.pid, "start": "1.000000"}, signal.SIGKILL) and pid_alive(p.pid),
          "a stale identity for a live pid (what pid reuse looks like) neither counts as alive nor receives a signal")
    check(G.safe_kill(good, signal.SIGKILL) and wait_until(lambda: not pid_alive(p.pid), 3), "the exact identity does")
    p.wait()
    check(not G.safe_kill({"pid": 1, "start": "1.000000"}, signal.SIGKILL) and not G.safe_kill({"pid": os.getpid(), "start": G.ident(os.getpid())["start"]}, signal.SIGKILL),
          "pid 1 and the guard itself are never signalled")

def s_ancestor():  # a process several levels up (the runner) is never signalled, even with an exact identity
    u = W + "/anc_unit.py"
    open(u, "w").write(f'import importlib.util, signal, sys\nspec = importlib.util.spec_from_file_location("g", {GUARD!r}); g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)\n'
                       'pid = int(sys.argv[1]); sys.exit(0 if not g.safe_kill(g.ident(pid), signal.SIGKILL) else 3)\n')
    subprocess.run(["sh", "-c", f"sh -c 'python3 {u} $PPID'; echo $? > {W}/anc.rc; echo survived > {W}/anc.out"], capture_output=True, text=True, env=ENV)
    check(os.path.exists(W + "/anc.out") and open(W + "/anc.rc").read().strip() == "0", "the guard's grandparent (outer shell) survives an exact-identity kill request")

def s_stale_ledger():
    metrics(); led = W + "/ledger"; os.makedirs(led, mode=0o700, exist_ok=True)
    dead = subprocess.Popen(["sleep", "0.1"]); dead_ident = G.ident(dead.pid); dead.wait()
    rd = W + "/stale"; os.makedirs(rd, exist_ok=True)
    e_pid, e = spawn_engine(rd + "/runtime.json", "normal", rd)
    base = {"id": "stale" + "0" * 27, "guard": dead_ident, "created": time.time(), "hard_epoch": 0, "shutdown_grace": 1, "cleanup_grace": 1,
            "run_dir": rd, "runtime_json": None, "cmd": None, "pgid": None, "private": True, "reaper": None}
    json.dump(dict(base, engines=[{"pid": e_pid, "start": "1.000000"}]), open(led + "/reused.json", "w"))  # pid reuse
    r = subprocess.run([sys.executable, GUARD, "--sweep"], capture_output=True, text=True, env=ENV)
    check(r.returncode == 0 and pid_alive(e_pid), "a ledger entry whose start time does not match (pid reuse) never signals the live process")
    json.dump(dict(base, engines=[G.ident(e_pid)]), open(led + "/orphan.json", "w"))  # owner died, engine survived
    subprocess.run([sys.executable, GUARD, "--sweep"], capture_output=True, text=True, env=ENV)
    check(wait_until(lambda: not pid_alive(e_pid), 5), "an engine orphaned by a dead guard is cleaned up by the next sweep")
    check(not os.path.exists(led + "/orphan.json"), "and its ledger entry is removed")
    check(unrelated is not None and pid_alive(unrelated.pid), "an unrelated engine-looking process was not touched")
    e.kill()

def s_protected():
    metrics(); rd = W + "/prot"; os.makedirs(rd, exist_ok=True)
    cmd = f'python3 {W}/actions-runner/hpt-fake-engine.py "$HARPOON_TEST_TMPDIR/runtime.json" normal "$HARPOON_TEST_TMPDIR" </dev/null >/dev/null 2>&1 & ' \
          + WAIT_RJ.format(rj="$HARPOON_TEST_TMPDIR/runtime.json") + f'cp "$HARPOON_TEST_TMPDIR/runtime.json" {W}/prot.json; exit 0'
    rc, err, _ = guard(["--deadline", "20", "--diag", W + "/prot.diag"] + FAST, sh(cmd))
    pid = pid_of(W + "/prot.json")
    check(pid_alive(pid), "a process that looks like a GitHub Actions runner is never signalled")
    check(rc == 71, f"and the run reports it could not verify cleanup (rc {rc})")
    os.kill(pid, signal.SIGKILL)

def s_concurrent():
    metrics(); results = {}
    def run(name, script, dl): results[name] = guard(["--deadline", dl] + FAST, sh(script), timeout=60)
    own = lambda n: engine_cmd("normal", f"{W}/c{n}.json")
    threads = [threading.Thread(target=run, args=("t1", own(1) + "sleep 30", "2")),
               threading.Thread(target=run, args=("t2", own(2) + f'sleep 3.5; kill -0 $(python3 -c "import json;print(json.load(open(\'{W}/c2.json\'))[\'pid\'])") && echo alive > {W}/c2.alive', "20")),
               threading.Thread(target=run, args=("t3", own(3) + "sleep 3.5; exit 0", "20"))]
    for t in threads: t.start()
    for t in threads: t.join()
    check(results["t1"][0] == 124, f"run 1 hits its own deadline ({results['t1'][0]})")
    check(results["t2"][0] == 0 and results["t3"][0] == 0, "concurrent runs 2 and 3 are unaffected")
    check(os.path.exists(W + "/c2.alive"), "run 2's engine stayed alive after run 1's cleanup")
    check(all(not pid_alive(pid_of(f"{W}/c{n}.json")) for n in (1, 2, 3)), "every run's own engine is gone at the end")

def watch_ledger(timeout=5):
    wait_until(lambda: ledger_files(), timeout); time.sleep(0.4)
    return json.load(open(W + "/ledger/" + ledger_files()[0]))

def s_guard_killed():
    metrics(); pf = W + "/gk.json"
    g = subprocess.Popen([sys.executable, GUARD, "--deadline", "100", "--shutdown-grace", "1.5", "--cleanup-grace", "1.5", "--admission-window", "0.3"] + ["--"] +
                         sh(engine_cmd("ignore", pf) + f"echo $$ > {W}/gk.cmd; exec sleep 100"), env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: os.path.exists(pf) and os.path.exists(W + "/gk.cmd"))
    epid, cpid = pid_of(pf), int(open(W + "/gk.cmd").read())
    time.sleep(0.6); g.kill(); g.wait()  # runner termination: the guard dies without any cleanup
    ok = wait_until(lambda: not pid_alive(epid) and not pid_alive(cpid), 12)
    check(ok, "after SIGKILL of the guard the detached reaper stops the command and the SIGTERM-ignoring engine")
    for p in (epid, cpid):
        try: os.kill(p, signal.SIGKILL)
        except OSError: pass

def s_reaper_respawn():  # the watchdog is itself supervised: kill it, the guard replaces it, then killing the guard still cleans up
    metrics(); pf = W + "/rr.json"
    g = subprocess.Popen([sys.executable, GUARD, "--deadline", "100", "--shutdown-grace", "1.5", "--cleanup-grace", "1.5", "--admission-window", "0.3"] + ["--"] +
                         sh(engine_cmd("normal", pf) + f"echo $$ > {W}/rr.cmd; exec sleep 100"), env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: os.path.exists(pf) and os.path.exists(W + "/rr.cmd"))
    epid, cpid = pid_of(pf), int(open(W + "/rr.cmd").read())
    first = watch_ledger()["reaper"]["pid"]
    os.kill(first, signal.SIGKILL)
    second = [None]
    def replaced():
        r = watch_ledger(1)["reaper"]; second[0] = r["pid"]; return r["pid"] != first and pid_alive(r["pid"])
    check(wait_until(replaced, 8), "a SIGKILLed reaper is replaced by the guard")
    g.kill(); g.wait()
    check(wait_until(lambda: not pid_alive(epid) and not pid_alive(cpid), 12), "and the replacement reaper cleans up after the guard dies")
    for p in (epid, cpid, second[0] or 0):
        try: os.kill(p, signal.SIGKILL)
        except OSError: pass

def s_startup_race():  # the guard dies after starting the command but before recording it: the command must never have run
    metrics(); started = W + "/sr.started"
    env = dict(ENV, VMGUARD_TEST_PAUSE_AFTER_SPAWN="4")
    g = subprocess.Popen([sys.executable, GUARD, "--deadline", "100", "--shutdown-grace", "1.5", "--cleanup-grace", "1.5", "--admission-window", "0.3"] + ["--"] +
                         sh(f"touch {started}; exec sleep 101"), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: ledger_files(), 5); time.sleep(0.8)
    check(not os.path.exists(started), "the command is gated: it has not started while the guard is still recording it")
    g.kill(); g.wait()
    time.sleep(5)  # past the pause the guard would have released the command after
    check(not os.path.exists(started), "a guard that dies in that window leaves no unrecorded command running (the gate pipe closes)")
    out = subprocess.run(["ps", "-axo", "command="], capture_output=True, text=True).stdout
    check(f"touch {started}" not in out, "and no gate process lingers")

def s_interrupt():
    metrics(); pf = W + "/it.json"
    g = subprocess.Popen([sys.executable, GUARD, "--deadline", "100"] + FAST + ["--"] + sh(engine_cmd("normal", pf) + "exec sleep 100"), env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_until(lambda: os.path.exists(pf)); time.sleep(0.6); g.send_signal(signal.SIGTERM)
    try: rc = g.wait(timeout=20)
    except subprocess.TimeoutExpired: g.kill(); rc = -1
    check(rc == 143, f"SIGTERM to the guard exits 128+15 (got {rc})"); check(not pid_alive(pid_of(pf)), "and the owned engine is gone")

def s_preflight():
    metrics(disk=1); rc, err, _ = guard(["--deadline", "5", "--diag", W + "/pf.diag"] + FAST, sh(f"touch {W}/pf.ran"))
    check(rc == 75 and not os.path.exists(W + "/pf.ran"), f"low disk refuses to start the test (rc {rc})")
    metrics(pressure=2); rc, _, _ = guard(["--deadline", "5"] + FAST, sh(f"touch {W}/pf.ran"))
    check(rc == 75, "elevated memory pressure NOW refuses to start")
    metrics(swap_used=9.9 * 2**30, swap_total=10 * 2**30, swap_out=50 * 2**30)  # swap almost full from the past, host calm now
    rc, _, _ = guard(["--deadline", "5"] + FAST, sh("exit 0"))
    check(rc == 0, f"historical swap occupancy alone does not refuse a calm host (rc {rc})")
    stop = threading.Event()
    def churn():
        used = 1 * 2**30
        while not stop.is_set():
            used += 300 * 2**20; metrics(swap_used=used); time.sleep(0.05)
    metrics(swap_used=1 * 2**30); t = threading.Thread(target=churn); t.start()
    rc, err, _ = guard(["--deadline", "5", "--diag", W + "/pf.diag"] + FAST, sh(f"touch {W}/pf2.ran"))
    stop.set(); t.join()
    check(rc == 75 and "swapping right now" in err and not os.path.exists(W + "/pf2.ran"), f"swapping during the admission window refuses to start (rc {rc})")

def s_pressure():
    metrics(); pf = W + "/pr.json"; m = W + "/metrics.json"
    sustained = engine_cmd("normal", pf) + f"""python3 -c "import json;json.dump({{'disk_free':100*2**30,'pressure':4,'swap_used':0,'swap_total':2**33}},open('{m}','w'))"; sleep 20"""
    rc, err, el = guard(["--deadline", "30", "--pressure-samples", "3", "--diag", W + "/pr.diag"] + FAST, sh(sustained))
    check(rc == 70 and el < 10, f"sustained critical memory pressure aborts (rc {rc}, {el:.1f}s)"); check(not pid_alive(pid_of(pf)), "and the owned engine is stopped")
    metrics()
    blip = f"""python3 - <<'EOF'
import json, time
f = {m!r}
w = lambda p: json.dump({{'disk_free': 100*2**30, 'pressure': p, 'swap_used': 0, 'swap_total': 2**33}}, open(f, 'w'))
w(4); time.sleep(0.3); w(1); time.sleep(1.5)
EOF"""
    check(guard(["--deadline", "20", "--pressure-samples", "3"] + FAST, sh(blip))[0] == 0, "a one-sample pressure blip does not abort")
    metrics()
    low = f"""python3 -c "import json;json.dump({{'disk_free':2**30,'pressure':1,'swap_used':0,'swap_total':2**33}},open('{m}','w'))"; sleep 20"""
    rc, err, el = guard(["--deadline", "30", "--diag", W + "/lo.diag"] + FAST, sh(low))
    check(rc == 70 and "disk" in err and el < 10, f"critically low disk aborts immediately (rc {rc}, {el:.1f}s)")
    def grow(pressure):
        return f"""python3 - <<'EOF'
import json, time
f = {m!r}; used = 2 * 2**30
for _ in range(40):
    json.dump({{'disk_free': 100*2**30, 'pressure': {pressure}, 'swap_used': used, 'swap_total': 20 * 2**30}}, open(f, 'w')); used += 300 * 2**20; time.sleep(0.1)
EOF"""
    metrics(swap_used=1 * 2**30)
    rc, err, _ = guard(["--deadline", "30", "--diag", W + "/sw.diag"] + FAST, sh(grow(2)))
    check(rc == 70 and "swap" in err, f"sustained swap growth under memory pressure aborts (rc {rc})")
    metrics(swap_used=1 * 2**30)
    rc, err, _ = guard(["--deadline", "30"] + FAST, sh(grow(1)))
    check(rc == 0, f"the same growth on a calm host (a VM legitimately growing swap) does not abort (rc {rc})")

def s_diag_bounded():
    metrics()
    cmd = f'head -c 5242880 /dev/zero | tr "\\0" "x" > "$HARPOON_TEST_TMPDIR/harpoon.log"; echo "$HARPOON_TEST_TMPDIR" > {W}/dg.ns; exit 1'
    rc, err, _ = guard(["--deadline", "20", "--diag", W + "/dg.diag"] + FAST, sh(cmd))
    ns = open(W + "/dg.ns").read().strip()
    check(rc == 1 and os.path.getsize(W + "/dg.diag") <= 8192, f"failure diagnostics are capped ({os.path.getsize(W + '/dg.diag')} bytes) though the log is 5 MiB")
    check(os.path.getsize(ns + "/harpoon.log") == 5 * 2**20, "and the original log is untouched (never copied or truncated)")
    shutil.rmtree(ns, ignore_errors=True)

def s_helper_leak():
    metrics(); hf = W + "/hl.pid"
    leak = f'env -u HARPOON_VMGUARD_ID python3 {HELPER} </dev/null >/dev/null 2>&1 & echo $! > {hf}; sleep 0.3; exit 0'
    rc, err, _ = guard(["--deadline", "20", "--diag", W + "/hl.diag"] + FAST, sh(leak))
    pid = int(open(hf).read())
    check(rc == 71 and "helper" in err, f"a VM helper still alive after the run is a failure (rc {rc})")
    check(pid_alive(pid), "and it is reported, not killed (it cannot be attributed with certainty)")
    os.kill(pid, signal.SIGKILL)
    clean = f'env -u HARPOON_VMGUARD_ID python3 {HELPER} </dev/null >/dev/null 2>&1 & p=$!; sleep 0.3; kill $p; sleep 0.3; exit 0'
    check(guard(["--deadline", "20"] + FAST, sh(clean))[0] == 0, "a helper the test itself stopped is fine")

SCENARIOS = {k[2:]: v for k, v in list(globals().items()) if k.startswith("s_") and callable(v)}
if __name__ == "__main__":
    names = sys.argv[1:] or list(SCENARIOS)
    before = set(os.listdir("/tmp"))
    unrelated = subprocess.Popen([sys.executable, ENGINE, W + "/unrelated.json", "normal", W], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for n in names:
            print(f"{n}:")
            try: SCENARIOS[n]()
            except Exception as e: check(False, f"{n} raised {type(e).__name__}: {e}")
        check(pid_alive(unrelated.pid), "the unrelated engine-looking process survived every scenario")
    finally:
        out = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout  # only processes this run created
        for line in out.splitlines():
            if TAG in line and "regression-vm-test-guard" not in line and ("hpt-fake" in line or "sleep 100" in line):
                try: os.kill(int(line.split()[0]), signal.SIGKILL)
                except OSError: pass
        shutil.rmtree(W, ignore_errors=True)
        for d in set(os.listdir("/tmp")) - before:  # private namespaces this run's failure cases kept for inspection
            if d.startswith("hpg."): shutil.rmtree("/tmp/" + d, ignore_errors=True)
    print("vm test guard: " + ("PASS" if not failures else f"{len(failures)} FAILED"))
    sys.exit(1 if failures else 0)
