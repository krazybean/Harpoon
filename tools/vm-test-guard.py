#!/usr/bin/env python3
"""Containment for VM-backed tests. Runs a test command under wall-clock deadlines and host-resource supervision, from a process that is
independent of Harpoon (a blocked Harpoon main queue cannot stop it), and signals only processes THIS run provably owns.

  vm-test-guard.py [--deadline S] [--namespace private|production] [options] -- command [args...]
  vm-test-guard.py --sweep                      clean up what dead guards (killed runners) left behind
  (internal) vm-test-guard.py --reap LEDGER     detached watchdog for a ledger, acts if the guard dies or overruns

Ownership, in order of strength:
  * a per-run marker (HARPOON_VMGUARD_ID) in the environment of the command, inherited by everything it starts, including engines
    that detach (`harpoon start` leaves PPID 1) and engines in a namespace the test chose itself (its own HARPOON_TEST_TMPDIR);
  * the command's process group (members started after the guard);
  * engines recorded in the namespace's runtime.json that started after the guard.
Every signal is sent to an identity (pid + microsecond start time from libproc) re-verified immediately before, never to a bare pid, and
never to pid <= 1, this process, any ancestor of it (the GitHub Actions runner), or anything that looks like a runner process.
The production namespace is exclusive: the guard refuses to run beside another test run or beside any already-running engine.
Production VMs themselves have no uptime limit; this only supervises test runs.

Exit: the command's own status, or 124 deadline, 75 refused (host busy / production namespace in use), 70 aborted on host pressure,
71 cleanup could not be verified. Not guaranteed (see docs/testing-vm-containment.md): SIGKILL of the guard AND its reaper, reboot,
processes that scrub their environment and leave the group, helpers that cannot be attributed to this run."""
import argparse, ctypes, ctypes.util, fcntl, glob, json, os, re, shutil, signal, struct, subprocess, sys, tempfile, time, uuid

EX_DEADLINE, EX_REFUSED, EX_ABORT, EX_CLEANUP = 124, 75, 70, 71
GIB, MIB = 1 << 30, 1 << 20
TEST = os.environ.get("VMGUARD_TEST") == "1"  # test-only hooks (fake metrics, engine/helper names, no-op SIGKILL, ...) are inert otherwise
ENGINE_MATCH = os.environ.get("VMGUARD_ENGINE_MATCH", "harpoon") if TEST else "harpoon"
HELPER_MATCH = os.environ.get("VMGUARD_HELPER_MATCH", "com.apple.Virtualization.VirtualMachine") if TEST else "com.apple.Virtualization.VirtualMachine"
PROTECTED = re.compile(r"Runner\.Listener|Runner\.Worker|actions-runner|runsvc|/sbin/launchd|/sbin/init", re.I)
DIAG_CAP = int(os.environ.get("VMGUARD_DIAG_CAP", 256 * 1024)) if TEST else 256 * 1024
NO_SIGKILL = TEST and os.environ.get("VMGUARD_TEST_SIGKILL_NOOP") == "1"
ACTIVE_SWAP = 64 * MIB  # swap-out or swap growth inside the admission window that counts as "swapping right now"
_libproc = ctypes.CDLL(ctypes.util.find_library("proc") or "libproc.dylib")


def say(msg): print(f"vmguard: {msg}", file=sys.stderr, flush=True)


# --- process identity (libproc: exact, microsecond start time, no pid-only decisions) ----------------------------------------------
def bsd(pid):
    buf = ctypes.create_string_buffer(136)  # struct proc_bsdinfo
    if pid <= 0 or _libproc.proc_pidinfo(pid, 3, 0, buf, 136) != 136: return None
    status, = struct.unpack_from("I", buf, 4); ppid, = struct.unpack_from("I", buf, 16); pgid, = struct.unpack_from("I", buf, 100)
    sec, usec = struct.unpack_from("QQ", buf, 120)
    return {"status": status, "ppid": ppid, "pgid": pgid, "start": f"{sec}.{usec:06d}"}

def ident(pid):
    b = bsd(pid)
    return {"pid": pid, "start": b["start"]} if b else None

def alive(i):
    b = bsd(i["pid"]) if i else None
    return bool(b) and b["start"] == i["start"] and b["status"] != 5  # 5 = SZOMB: a zombie is dead

def start_epoch(i): return float(i["start"])

def command(pid):
    r = subprocess.run(["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""

def is_engine(cmdline):
    tokens = cmdline.split()
    return bool(tokens) and (os.path.basename(tokens[0]) == "harpoon" or (TEST and ENGINE_MATCH in " ".join(tokens[:2])))

def ancestors():
    out, pid = set(), os.getpid()
    while pid > 1:
        b = bsd(pid)
        if not b or b["ppid"] in out: break
        out.add(b["ppid"]); pid = b["ppid"]
    return out

def safe_kill(i, sig):
    """Signal only a verified identity; never pid<=1, this process, an ancestor (the runner), or a runner-looking process."""
    if not i or i["pid"] <= 1 or i["pid"] == os.getpid() or i["pid"] in ancestors(): return False
    if PROTECTED.search(command(i["pid"])): return False
    if not alive(i): return False  # checked last, as close to the kill as possible: a reused pid has a different start time
    try: os.kill(i["pid"], sig); return True
    except OSError: return False

def wait_dead(items, seconds):
    items = items if isinstance(items, list) else [items]
    end = time.time() + seconds
    while any(alive(i) for i in items) and time.time() < end: time.sleep(0.1)
    return not any(alive(i) for i in items)

def marked(run_id, exclude=()):
    """Every process whose environment carries this run's marker, as (identity, command line)."""
    out = subprocess.run(["ps", "-axwwE", "-o", "pid=,command="], capture_output=True, text=True).stdout
    tok = f" HARPOON_VMGUARD_ID={run_id}"
    found = []
    for line in out.splitlines():
        if tok not in line: continue
        parts = line.strip().split(None, 1)
        if not parts or not parts[0].isdigit() or int(parts[0]) in exclude or int(parts[0]) == os.getpid(): continue
        i = ident(int(parts[0]))
        if i: found.append((i, parts[1].split(tok)[0] if len(parts) > 1 else ""))
    return found

def group_members(pgid, since):
    found = []
    for p in subprocess.run(["ps", "-axo", "pid="], capture_output=True, text=True).stdout.split():
        b = bsd(int(p))
        if b and b["pgid"] == pgid and float(b["start"]) >= since - 1: found.append({"pid": int(p), "start": b["start"]})
    return found

def helpers():
    out = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    return {int(l.split(None, 1)[0]) for l in out.splitlines() if HELPER_MATCH in l and "vm-test-guard" not in l and l.split(None, 1)[0].isdigit()}


# --- ledger ----------------------------------------------------------------------------------------------------------------------
def ledger_dir():
    d = (os.environ.get("VMGUARD_LEDGER_DIR") if TEST else None) or f"/tmp/harpoon-vmguard-{os.getuid()}"
    os.makedirs(d, mode=0o700, exist_ok=True)
    st = os.lstat(d)
    if st.st_uid != os.getuid() or (st.st_mode & 0o022) or os.path.islink(d): sys.exit(f"vmguard: {d} is not a private directory owned by this user")
    return d

def save(path, led):
    tmp = path + ".tmp"
    with open(tmp, "w") as f: json.dump(led, f)
    os.replace(tmp, path)

def load(path):
    try:
        with open(path) as f: return json.load(f)
    except (OSError, ValueError): return None

def runtime_files(led):
    if led.get("runtime_json"): return [led["runtime_json"]]
    ns = led["run_dir"]
    return [ns + "/runtime.json"] + sorted(glob.glob(ns + "/*/runtime.json")) + sorted(glob.glob(ns + "/*/*/runtime.json"))

def runtime_engine(led):
    for p in runtime_files(led):
        d = load(p)
        if d and isinstance(d.get("pid"), int): return d["pid"]
    return None

def owned(led):
    """Everything this run owns that is still alive: recorded engines, marker holders, the command's process group."""
    items = {(e["pid"], e["start"]): e for e in led["engines"]}
    ex = {(led.get("reaper") or {}).get("pid")}
    for i, _ in marked(led["id"], ex): items[(i["pid"], i["start"])] = i
    if led.get("pgid"):
        for m in group_members(led["pgid"], led["created"]): items[(m["pid"], m["start"])] = m
    return [i for i in items.values() if alive(i)]

def stop_engine(i, stopfile, shutdown_grace, cleanup_grace):
    """Escalation: the engine's own stop file and SIGTERM both need its main queue; SIGKILL does not."""
    if not alive(i): return True
    if stopfile:
        try: open(stopfile, "w").close()
        except OSError: pass
    if wait_dead(i, shutdown_grace / 2): return True
    safe_kill(i, signal.SIGTERM)
    if wait_dead(i, shutdown_grace / 2): return True
    if not NO_SIGKILL: safe_kill(i, signal.SIGKILL)
    return wait_dead(i, cleanup_grace)

def terminate(items, sg, cg):
    for i in items: safe_kill(i, signal.SIGTERM)
    if wait_dead(items, sg / 2): return
    if not NO_SIGKILL:
        for i in items: safe_kill(i, signal.SIGKILL)
    wait_dead(items, cg)

def cleanup(led):
    """Stop everything this ledger owns. Returns the identities still alive afterwards."""
    sg, cg = led["shutdown_grace"], led["cleanup_grace"]
    stopfile = (led["run_dir"] + "/harpoon-stop") if led.get("run_dir") else None
    is_eng = lambda i: i in led["engines"] or is_engine(command(i["pid"]))
    current = owned(led)
    terminate([i for i in current if not is_eng(i)], sg, cg)          # 1. the command and its helpers
    for e in [i for i in owned(led) if is_eng(i)]: stop_engine(e, stopfile, sg, cg)  # 2. engines: stop file, SIGTERM, SIGKILL
    left = owned(led)
    if left: terminate(left, sg, cg)                                     # 3. anything started in the meantime
    return owned(led)


# --- host resources --------------------------------------------------------------------------------------------------------------
def metrics(path):
    f = os.environ.get("VMGUARD_METRICS_FILE") if TEST else None
    if f:
        try: return json.load(open(f))
        except (OSError, ValueError): pass
    st = os.statvfs(path)
    def run(*c): return subprocess.run(list(c), capture_output=True, text=True).stdout
    swap = {k: float(v) * MIB for k, v in re.findall(r"(used|total) = ([\d.]+)M", run("sysctl", "-n", "vm.swapusage"))}
    pressure = run("sysctl", "-n", "kern.memorystatus_vm_pressure_level").strip()
    vm = run("vm_stat"); page = re.search(r"page size of (\d+)", vm); outs = re.search(r"Swapouts:\s+(\d+)", vm)
    return {"disk_free": st.f_bavail * st.f_frsize, "pressure": int(pressure) if pressure.isdigit() else 1,
            "swap_used": swap.get("used", 0), "swap_total": swap.get("total", 0),
            "swap_out": int(outs.group(1)) * int(page.group(1)) if page and outs else 0}

def admission(a, path):
    """Current conditions only: free disk, memory pressure now, and swapping happening NOW. Swap occupancy alone says nothing: macOS keeps
    swap files allocated long after the pressure that created them is gone, and grows them on demand."""
    m1 = metrics(path)
    if m1["disk_free"] < a.min_disk_start: return f"free disk {m1['disk_free'] / GIB:.1f} GiB is below the {a.min_disk_start / GIB:.0f} GiB needed to start"
    if m1["pressure"] >= 2: return f"memory pressure level {m1['pressure']} (normal is 1)"
    time.sleep(a.admission_window)
    m2 = metrics(path)
    if m2["pressure"] >= 2: return f"memory pressure level {m2['pressure']} (normal is 1)"
    moved = max(m2["swap_out"] - m1["swap_out"], m2["swap_used"] - m1["swap_used"])
    if moved >= ACTIVE_SWAP: return f"the host is swapping right now ({moved / MIB:.0f} MiB in {a.admission_window:g}s)"
    return None

class Watch:
    def __init__(self, a): self.a, self.crit, self.swaps = a, 0, []
    def check(self, m):
        a = self.a
        if m["disk_free"] < a.min_disk_abort: return f"free disk {m['disk_free'] / GIB:.1f} GiB is critically low (< {a.min_disk_abort / GIB:.0f} GiB)"
        self.crit = self.crit + 1 if m["pressure"] >= 4 else 0
        if self.crit >= a.pressure_samples: return f"critical memory pressure for {self.crit} consecutive samples"
        self.swaps = (self.swaps + [m["swap_used"]])[-(a.swap_samples + 1):]
        # sustained growth only counts while the host is under pressure NOW; a VM legitimately grows swap on a calm host
        if m["pressure"] >= 2 and len(self.swaps) > a.swap_samples and all(y - x >= a.swap_growth for x, y in zip(self.swaps, self.swaps[1:])):
            return f"swap growing >= {a.swap_growth / MIB:.0f} MiB per sample for {a.swap_samples} samples under memory pressure"
        return None


# --- bounded diagnostics ---------------------------------------------------------------------------------------------------------
def tail(path, n):
    try:
        with open(path, "rb") as f:
            f.seek(0, 2); f.seek(max(0, f.tell() - n)); return f.read().decode("utf-8", "replace")
    except OSError: return "(unreadable)"

def write_diag(path, reason, led, samples, left):
    parts = [f"reason: {reason}", f"owned engines: {led['engines']}", f"still alive: {left}", "recent samples:\n" + "\n".join(samples[-20:])]
    rd = led.get("run_dir")
    if rd:
        for name in ("harpoon.log", "harpoon-serial.log", "harpoon-serial.log.1", "runtime.json"):
            p = rd + "/" + name
            if os.path.exists(p): parts.append(f"--- tail of {name} (16 KiB max)\n" + tail(p, 16 * 1024))
    with open(path, "wb") as f: f.write("\n".join(parts).encode("utf-8", "replace")[:DIAG_CAP])  # the only file written; never a copy of a whole log
    return path


# --- watchdogs -------------------------------------------------------------------------------------------------------------------
def spawn_reaper(lpath):
    return subprocess.Popen([sys.executable, os.path.abspath(__file__), "--reap", lpath], start_new_session=True,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)  # own session AND group; inherits no lock fds

def reap(path):
    while True:
        led = load(path)
        if not led: return 0
        over = time.time() >= led["hard_epoch"]
        if over or not alive(led["guard"]):
            fd = os.open(path, os.O_RDONLY)
            try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError: return 0  # a sweep is already cleaning up
            if alive(led["guard"]): safe_kill(led["guard"], signal.SIGKILL)  # overran its own hard deadline
            say(f"reaper: {'guard overran' if over else 'guard died'}; cleaning up {path}")
            cleanup(led)
            try: os.unlink(path)
            except OSError: pass
            return 0
        time.sleep(1)

def sweep(d):
    n = 0
    for name in sorted(os.listdir(d)):
        p = os.path.join(d, name)
        led = load(p) if name.endswith(".json") else None
        if not led or alive(led["guard"]): continue
        fd = os.open(p, os.O_RDONLY)
        try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError: continue
        say(f"sweep: guard {led['guard']['pid']} is gone; cleaning up what it owned")
        cleanup(led); n += 1
        try: os.unlink(p)
        except OSError: pass
    return n

def production_exclusive(d, led, adopt):
    """The production namespace is shared by every test and by the user's own Harpoon: take it alone or not at all."""
    fd = os.open(os.path.join(d, "production.lock"), os.O_CREAT | os.O_RDWR, 0o600)  # held for the whole run; Python fds are not inherited
    try: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)  # EXCLUSIVE_LOCK
    except OSError: return None, "another test run owns the production namespace"
    if not adopt:
        lock = led["run_dir"] + "/harpoon.lock"
        if os.path.exists(lock):
            lf = os.open(lock, os.O_RDONLY)
            try: fcntl.flock(lf, fcntl.LOCK_SH | fcntl.LOCK_NB)
            except OSError: return None, f"a Harpoon engine is already running ({lock} is held); tests will not share it (--adopt-existing overrides)"
            finally: os.close(lf)
        pid = runtime_engine(led)
        if pid and ident(pid) and is_engine(command(pid)): return None, f"a Harpoon engine (pid {pid}) is already running in the production namespace (--adopt-existing overrides)"
    return fd, None


def main():
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--deadline", type=float, default=1800, help="wall-clock seconds for startup + execution")
    ap.add_argument("--shutdown-grace", type=float, default=60, help="seconds for graceful engine stop (stop file, then SIGTERM)")
    ap.add_argument("--cleanup-grace", type=float, default=30, help="seconds to confirm exit after SIGKILL")
    ap.add_argument("--namespace", choices=["private", "production"], default="private")
    ap.add_argument("--adopt-existing", action="store_true", default=os.environ.get("VMGUARD_ADOPT_EXISTING") == "1",
                    help="production only: run beside an engine that is already running (it stays unowned and is never touched)")
    ap.add_argument("--run-dir"); ap.add_argument("--runtime-json")  # explicit overrides (tests, non-standard layouts)
    ap.add_argument("--sample-interval", type=float, default=15); ap.add_argument("--admission-window", type=float, default=3)
    ap.add_argument("--min-disk-start", type=float, default=15 * GIB); ap.add_argument("--min-disk-abort", type=float, default=8 * GIB)
    ap.add_argument("--swap-growth", type=float, default=256 * MIB)
    ap.add_argument("--pressure-samples", type=int, default=3); ap.add_argument("--swap-samples", type=int, default=3)
    ap.add_argument("--diag"); ap.add_argument("--sweep", action="store_true"); ap.add_argument("--reap")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    if a.reap: return reap(a.reap)
    d = ledger_dir()
    if a.sweep: say(f"swept {sweep(d)} stale run(s)"); return 0
    cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
    if not cmd: ap.error("no command")
    sweep(d)  # a previous runner that was killed must not leave this run to share the host with its VM

    private = a.namespace == "private" and not a.run_dir
    run_dir = a.run_dir or (tempfile.mkdtemp(prefix="hpg.", dir="/tmp") if private else "/tmp")
    if private: run_dir = os.path.realpath(run_dir)
    runtime_json = a.runtime_json or (None if private else os.path.expanduser("~/Library/Application Support/Harpoon/runtime.json"))
    watch_path = run_dir if os.path.isdir(run_dir) else "/tmp"
    led = {"id": uuid.uuid4().hex, "guard": ident(os.getpid()), "created": time.time(), "hard_epoch": time.time() + a.deadline + a.shutdown_grace + a.cleanup_grace + 10,
           "shutdown_grace": a.shutdown_grace, "cleanup_grace": a.cleanup_grace, "run_dir": run_dir, "runtime_json": runtime_json,
           "engines": [], "cmd": None, "pgid": None, "private": private, "reaper": None}

    def refuse(why):
        say(f"REFUSED to start the test: {why}")
        if private: shutil.rmtree(run_dir, ignore_errors=True)
        return EX_REFUSED
    prod_lock = None
    if not private:
        prod_lock, why = production_exclusive(d, led, a.adopt_existing)
        if why: return refuse(why)
    why = admission(a, watch_path)
    if why:
        if prod_lock is not None: os.close(prod_lock)
        return refuse(why)

    pre = runtime_engine(led) if not private else None
    preexisting = ident(pre) if pre else None
    lpath = os.path.join(d, f"{os.getpid()}-{int(time.time() * 1000)}.json")
    save(lpath, led)
    reaper = spawn_reaper(lpath); led["reaper"] = ident(reaper.pid); save(lpath, led)
    env = dict(os.environ, HARPOON_VMGUARD_ID=led["id"], HARPOON_VMGUARD_ACTIVE="1")
    if private: env["HARPOON_TEST_TMPDIR"] = run_dir
    helpers0 = helpers()
    # The command is gated: it blocks on a pipe until its identity is in the ledger, so a guard that dies first leaves nothing unrecorded
    # (the pipe closes, `read` fails, the child exits 125 without ever running test code).
    go_r, go_w = os.pipe()
    gate = 'eval "read go <&$VMGUARD_GO_FD" || exit 125; eval "exec $VMGUARD_GO_FD<&-"; unset VMGUARD_GO_FD; exec "$@"'  # GATE
    proc = subprocess.Popen(["/bin/sh", "-c", gate, "vmguard-gate"] + cmd, start_new_session=True, env=dict(env, VMGUARD_GO_FD=str(go_r)), pass_fds=(go_r,))
    os.close(go_r)
    led["cmd"], led["pgid"] = ident(proc.pid), proc.pid
    save(lpath, led)
    if TEST and os.environ.get("VMGUARD_TEST_PAUSE_AFTER_SPAWN"): time.sleep(float(os.environ["VMGUARD_TEST_PAUSE_AFTER_SPAWN"]))  # widens the window before release
    os.write(go_w, b"go\n"); os.close(go_w)

    state = {"sig": None}
    for s in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP): signal.signal(s, lambda n, f: state.__setitem__("sig", n))
    watch, samples, outcome, rc = Watch(a), [], None, None
    t0 = time.time(); next_sample = t0 + a.sample_interval; next_scan = t0

    def record(i):
        if not any(e["pid"] == i["pid"] and e["start"] == i["start"] for e in led["engines"]):
            led["engines"].append(i); save(lpath, led); say(f"owns engine pid {i['pid']}")
    def observe():
        nonlocal next_scan
        pid = runtime_engine(led)
        if pid:
            i = ident(pid)
            if i and is_engine(command(pid)) and start_epoch(i) >= led["created"] - 0.5 and i != preexisting: record(i)
        if time.time() >= next_scan:  # engines anywhere (a test-chosen namespace, detached) are found by the marker they inherited
            next_scan = time.time() + 2
            for i, line in marked(led["id"], {led["reaper"]["pid"]} if led["reaper"] else ()):
                if is_engine(line): record(i)

    while True:
        rc = proc.poll()
        observe()
        if reaper.poll() is not None:  # the watchdog must outlive the guard: replace it if it died
            reaper = spawn_reaper(lpath); led["reaper"] = ident(reaper.pid); save(lpath, led); say("reaper restarted")
        if rc is not None: break
        now = time.time()
        if state["sig"]: outcome = ("interrupted", 128 + state["sig"]); break
        if now - t0 > a.deadline: outcome = (f"deadline of {a.deadline:.0f}s exceeded", EX_DEADLINE); break
        if now >= next_sample:
            next_sample = now + a.sample_interval
            m = metrics(watch_path); samples.append(f"{time.strftime('%H:%M:%S')} {json.dumps(m)}")
            bad = watch.check(m)
            if bad: outcome = (f"aborted on host pressure: {bad}", EX_ABORT); break
        time.sleep(min(0.2, max(0.02, a.sample_interval / 4)))
    if outcome: say(outcome[0])
    next_scan = 0; observe()
    left = cleanup(led)
    leaked = set()
    end = time.time() + a.cleanup_grace
    while time.time() < end:
        leaked = helpers() - helpers0
        if not leaked or left: break
        time.sleep(0.2)
    final = outcome[1] if outcome else rc
    problem = None
    if left: problem = f"owned processes survived cleanup: {left}"
    elif leaked: problem = f"VM helper process(es) {sorted(leaked)} appeared during this run and are still alive (not signalled: they cannot be attributed to this run with certainty)"
    if problem: say(problem); final = EX_CLEANUP
    if final != 0:
        out = a.diag or os.path.join(tempfile.gettempdir(), f"vmguard-diag-{os.getpid()}.txt")
        say(f"failure diagnostics (bounded to {DIAG_CAP // 1024} KiB): {write_diag(out, problem or (outcome[0] if outcome else f'exit {rc}'), led, samples, left)}")
    elif private:
        shutil.rmtree(run_dir, ignore_errors=True)  # only the namespace this guard created, only after a verified clean run
    try: os.unlink(lpath)  # the reaper notices the missing ledger and exits
    except OSError: pass
    if prod_lock is not None: os.close(prod_lock)
    return final

if __name__ == "__main__":
    sys.exit(main())
