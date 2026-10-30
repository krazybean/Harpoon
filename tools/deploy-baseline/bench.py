#!/usr/bin/env python3
"""Harpoon 2.1 deployment baseline harness (Track A).

Disposable, repeatable measurements against a *running* Harpoon runtime. It never
starts, stops or reconfigures the VM, never prunes, and only creates Docker objects
labelled harpoon.baseline=1, which it removes itself.

Usage: bench.py <build|concurrent|publish|idle|cleanup> [--out DIR] [--n N]
"""
import argparse, csv, json, os, random, shutil, signal, socket, statistics, subprocess
import sys, threading, time, http.client, resource

ROOT = os.path.dirname(os.path.abspath(__file__))
HARPOON = os.environ.get("HARPOON_BIN", "/Users/juancastro/Documents/Github/Harpoon/harpoon/build/harpoon")
DOCKER = os.environ.get("DOCKER_BIN", "/opt/homebrew/bin/docker")
SOCK = "unix:///tmp/harpoon-docker.sock"
FX_HOST = "/private/tmp/harpoon-baseline-fx"          # visible to macOS
FX_GUEST = "/mnt/harpoon-host/tmp/harpoon-baseline-fx"  # same dir via VirtioFS
LABEL = "harpoon.baseline=1"
ENV = dict(os.environ, DOCKER_HOST=SOCK)
ENV.pop("DOCKER_CONTEXT", None)


def now():
    return time.monotonic()


def sh(argv, timeout=900, env=ENV, input_=None):
    t0 = now()
    p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout, env=env, input=input_)
    return p.returncode, p.stdout, p.stderr, now() - t0


def docker(*a, **k):
    return sh([DOCKER, *a], **k)


def gexec(script, timeout=60):
    return sh([HARPOON, "exec", "--", "sh", "-c", script], timeout=timeout)


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return None
    k = (len(xs) - 1) * q
    f, c = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def summ(xs):
    if not xs:
        return {"n": 0}
    return {"n": len(xs), "min": min(xs), "median": statistics.median(xs), "p95": pct(xs, .95),
            "max": max(xs), "mean": statistics.fmean(xs),
            "stdev": statistics.stdev(xs) if len(xs) > 1 else 0.0}


def env_info():
    rc, hv, _, _ = sh([HARPOON, "status", "--json"])
    st = json.loads(hv) if rc == 0 else {}
    rc, dv, _, _ = docker("version", "--format", "{{json .}}")
    srv = json.loads(dv).get("Server", {}) if rc == 0 else {}
    _, mem, _, _ = gexec("grep -E 'MemTotal|MemAvailable' /proc/meminfo; uname -r; nproc")
    _, df, _, _ = sh(["df", "-h", "/System/Volumes/Data"])
    _, sw, _, _ = sh(["sw_vers", "-productVersion"])
    _, cpu, _, _ = sh(["sysctl", "-n", "machdep.cpu.brand_string"])
    sha = subprocess.run(["shasum", "-a", "256", HARPOON], capture_output=True, text=True).stdout.split()[0]
    return {"date": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "macos": sw.strip(), "cpu": cpu.strip(),
            "harpoon_state": st, "harpoon_bin": HARPOON, "harpoon_bin_sha256": sha,
            "docker_server": {k: srv.get(k) for k in ("Version", "Os", "Arch")},
            "guest": mem.strip().splitlines(), "host_disk": df.strip().splitlines()[-1]}


# ---------------------------------------------------------------- sampler
GUEST_PROBE = (
    "grep -E '^(MemTotal|MemFree|MemAvailable|Cached|Dirty):' /proc/meminfo | tr -s ' ' | tr '\\n' ';'; echo; "
    "cat /proc/loadavg; head -1 /proc/stat; "
    "for p in dockerd containerd; do pid=$(pidof $p | cut -d' ' -f1); "
    "echo \"$p $(awk '/VmRSS/{print $2}' /proc/$pid/status 2>/dev/null)\"; done; "
    "cat /proc/pressure/memory 2>/dev/null | head -1; cat /proc/pressure/cpu 2>/dev/null | head -1"
)


def hostps(pid):
    rc, out, _, _ = sh(["ps", "-o", "rss=,time=", "-p", str(pid)])
    if rc != 0 or not out.strip():
        return None, None
    rss, cputime = out.split()
    m, s = cputime.split(":")
    return int(rss), int(m) * 60 + float(s)


class Sampler(threading.Thread):
    def __init__(self, path, harpoon_pid, vz_pid, interval=1.0):
        super().__init__(daemon=True)
        self.path, self.hp, self.vz, self.interval = path, harpoon_pid, vz_pid, interval
        self.stop_ev = threading.Event()
        self.prev = None
        self.t0 = now()

    def run(self):
        with open(self.path, "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["t_s", "mem_avail_kB", "mem_free_kB", "cached_kB", "dirty_kB", "load1", "guest_cpu_busy_pct",
                        "dockerd_rss_kB", "containerd_rss_kB", "psi_mem_some_avg10", "psi_cpu_some_avg10",
                        "host_harpoon_rss_kB", "host_harpoon_cpu_pct", "host_vz_rss_kB", "host_vz_cpu_pct"])
            hprev = vprev = None
            while not self.stop_ev.is_set():
                ts = now()
                rc, out, _, _ = gexec(GUEST_PROBE, timeout=20)
                row = [round(ts - self.t0, 2)] + [""] * 14
                if rc == 0:
                    lines = out.splitlines()
                    try:
                        mem = dict(kv.split(":") for kv in lines[0].strip(";").split(";") if ":" in kv)
                        g = lambda k: int(mem[k].split()[0])
                        row[1:5] = [g("MemAvailable"), g("MemFree"), g("Cached"), g("Dirty")]
                        row[5] = lines[1].split()[0]
                        cpu = list(map(int, lines[2].split()[1:8]))
                        busy, tot = sum(cpu) - cpu[3] - cpu[4], sum(cpu)
                        if self.prev:
                            dt = tot - self.prev[1]
                            row[6] = round(100 * (busy - self.prev[0]) / dt, 1) if dt else ""
                        self.prev = (busy, tot)
                        for l in lines[3:]:
                            if l.startswith("dockerd"):
                                row[7] = l.split()[1] if len(l.split()) > 1 else ""
                            elif l.startswith("containerd"):
                                row[8] = l.split()[1] if len(l.split()) > 1 else ""
                            elif l.startswith("some") and "avg10" in l:
                                v = l.split("avg10=")[1].split()[0]
                                row[9 if row[9] == "" else 10] = v
                    except Exception as e:  # keep sampling; record the failure
                        row[14] = f"parse-error:{e}"
                hr, hc = hostps(self.hp)
                vr, vc = hostps(self.vz)
                row[11] = hr if hr is not None else ""
                row[13] = vr if vr is not None else ""
                if hc is not None and hprev:
                    row[12] = round(100 * (hc - hprev[0]) / (ts - hprev[1]), 1)
                if vc is not None and vprev:
                    row[14] = round(100 * (vc - vprev[0]) / (ts - vprev[1]), 1)
                hprev = (hc, ts) if hc is not None else None
                vprev = (vc, ts) if vc is not None else None
                w.writerow(row); f.flush()
                self.stop_ev.wait(max(0.0, self.interval - (now() - ts)))

    def stop(self):
        self.stop_ev.set(); self.join(timeout=10)


def pids():
    rc, out, _, _ = sh([HARPOON, "status", "--json"])
    hp = json.loads(out)["pid"]
    rc, out, _, _ = sh(["pgrep", "-f", "com.apple.Virtualization.VirtualMachine"])
    vz = int(out.split()[0]) if out.split() else 0
    return hp, vz


# ---------------------------------------------------------------- fixtures
def stage_fixtures(ctx_mb=50):
    os.makedirs(FX_HOST, exist_ok=True)
    for name in ("fx-web", "fx-burn"):
        dst = os.path.join(FX_HOST, name)
        shutil.rmtree(dst, ignore_errors=True)
        shutil.copytree(os.path.join(ROOT, "fixtures", name), dst)
    dst = os.path.join(FX_HOST, "fx-ctx")
    shutil.rmtree(dst, ignore_errors=True)
    shutil.copytree(os.path.join(ROOT, "fixtures", "fx-web"), dst)
    with open(os.path.join(dst, "payload.bin"), "wb") as f:   # deterministic, incompressible-ish
        rnd = random.Random(2110)
        for _ in range(ctx_mb):
            f.write(rnd.randbytes(1 << 20))
    with open(os.path.join(dst, "Dockerfile"), "a") as f:
        f.write("COPY payload.bin /data/payload.bin\n")


def unstage():
    shutil.rmtree(FX_HOST, ignore_errors=True)


def build(fx, tag, args=(), cache=False, where="host"):
    """Return dict(rc, wall_s, log) for one build via the Harpoon socket (host) or inside the guest."""
    base = ["build", "--progress=plain", "--label", LABEL, "-t", tag]
    if not cache:
        base.append("--no-cache")
    for a in args:
        base += ["--build-arg", a]
    if where == "host":
        rc, out, err, wall = docker(*base, os.path.join(FX_HOST, fx), timeout=1800)
    else:
        # The guest Docker CLI has no buildx plugin: without DOCKER_BUILDKIT=1 it silently uses the
        # deprecated legacy builder, which is not comparable to the host (BuildKit) path.
        base = [b for b in base if b != "--progress=plain"]
        rc, out, err, wall = sh([HARPOON, "exec", "--", "env", "DOCKER_BUILDKIT=1", "BUILDKIT_PROGRESS=plain",
                                 "docker", *base, f"{FX_GUEST}/{fx}"], timeout=1800)
    return {"rc": rc, "wall_s": wall, "log": (out + err)}


def cleanup():
    rc, ids, _, _ = docker("ps", "-aq", "--filter", f"label={LABEL}")
    if ids.split():
        docker("rm", "-f", *ids.split())
    rc, ids, _, _ = docker("image", "ls", "-q", "--filter", f"label={LABEL}")
    if ids.split():
        docker("image", "rm", "-f", *set(ids.split()))
    unstage()


# ---------------------------------------------------------------- measurements
def m_build(out, n, only=None):
    stage_fixtures()
    hp, vz = pids()
    results = {}
    sampler = Sampler(os.path.join(out, "build-sampler.csv" if only != "guest" else "build-guest-sampler.csv"), hp, vz); sampler.start()
    marks = []
    try:
        plan = [("fx-web", [], False, "cold"), ("fx-web", [], True, "warm"),
                ("fx-ctx", [], False, "cold"), ("fx-burn", ["CPU_MB=2000", "MEM_MB=768"], False, "cold")]
        # warm-up pull-free: base image alpine:3.22 is already local
        for fx, args, cache, kind in ([] if only == "guest" else plan):
            key = f"{fx}/{kind}"
            runs = []
            if cache:  # prime the cache once, not counted
                build(fx, f"harpoon-baseline/{fx}:prime", args, cache=True)
            reps = n if fx in ("fx-web", "fx-burn") else max(3, n // 2)
            for i in range(reps):
                t_start = now() - sampler.t0
                r = build(fx, f"harpoon-baseline/{fx}:{kind}{i}", args, cache=cache)
                t_end = now() - sampler.t0
                marks.append((key, i, round(t_start, 2), round(t_end, 2), r["rc"]))
                with open(os.path.join(out, f"build-{fx}-{kind}-{i}.log"), "w") as f:
                    f.write(r["log"])
                runs.append(r)
                time.sleep(2)
            results[f"socket:{key}"] = {"wall_s": summ([r["wall_s"] for r in runs if r["rc"] == 0]),
                                         "failures": sum(r["rc"] != 0 for r in runs)}
        # same builds executed inside the guest (no macOS client / no socket hop)
        for fx, args in (("fx-web", []), ("fx-burn", ["CPU_MB=2000", "MEM_MB=768"])):
            runs = []
            for i in range(max(3, n // 2)):
                t_start = now() - sampler.t0
                r = build(fx, f"harpoon-baseline/{fx}:guest{i}", args, cache=False, where="guest")
                marks.append((f"guest:{fx}/cold", i, round(t_start, 2), round(now() - sampler.t0, 2), r["rc"]))
                with open(os.path.join(out, f"build-guest-{fx}-{i}.log"), "w") as f:
                    f.write(r["log"])
                runs.append(r)
                time.sleep(2)
            results[f"guest:{fx}/cold"] = {"wall_s": summ([r["wall_s"] for r in runs if r["rc"] == 0]),
                                           "failures": sum(r["rc"] != 0 for r in runs)}
    finally:
        sampler.stop()
    with open(os.path.join(out, "build-marks.csv" if only != "guest" else "build-guest-marks.csv"), "w", newline="") as f:
        w = csv.writer(f); w.writerow(["case", "iter", "t_start_s", "t_end_s", "rc"]); w.writerows(marks)
    return results


def load_gen(port, stop_ev, rows, interval=0.05):
    t0 = now()
    while not stop_ev.is_set():
        ts = now()
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
            c.request("GET", "/index.html"); r = c.getresponse(); r.read(); c.close()
            rows.append((ts - t0, (now() - ts) * 1000, r.status))
        except Exception as e:
            rows.append((ts - t0, (now() - ts) * 1000, type(e).__name__))
        stop_ev.wait(max(0.0, interval - (now() - ts)))


def phase_stats(rows, a, b):
    sel = [r for r in rows if a <= r[0] < b]
    ok = [r[1] for r in sel if r[2] == 200]
    return {"requests": len(sel), "errors": len(sel) - len(ok),
            "p50_ms": pct(ok, .5), "p95_ms": pct(ok, .95), "p99_ms": pct(ok, .99), "max_ms": max(ok) if ok else None}


def m_concurrent(out, n):
    stage_fixtures()
    hp, vz = pids()
    port = 38081
    rc, *_ = docker("build", "-q", "--label", LABEL, "-t", "harpoon-baseline/fx-web:app", os.path.join(FX_HOST, "fx-web"))
    rc, o, e, _ = docker("run", "-d", "--name", "baseline-app", "--label", LABEL, "-p", f"{port}:8080",
                         "harpoon-baseline/fx-web:app")
    if rc != 0:
        raise SystemExit("could not start app: " + e)
    for _ in range(300):  # wait for publish
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close(); break
        except OSError:
            time.sleep(0.1)
    results = {}
    for name, args in (("cpu2x_mem768", ["CPU_MB=2000", "MEM_MB=768"]), ("cpu2x_mem1536", ["CPU_MB=2000", "MEM_MB=1536"])):
        _, oom0, _, _ = gexec("dmesg 2>/dev/null | grep -ci 'out of memory\\|oom-kill' || true")
        sampler = Sampler(os.path.join(out, f"concurrent-{name}-sampler.csv"), hp, vz); sampler.start()
        stop_ev, rows = threading.Event(), []
        lg = threading.Thread(target=load_gen, args=(port, stop_ev, rows), daemon=True); lg.start()
        time.sleep(30)
        b0 = rows[-1][0] if rows else 0
        r = build("fx-burn", f"harpoon-baseline/fx-burn:{name}", args, cache=False)
        b1 = rows[-1][0] if rows else b0
        time.sleep(30)
        stop_ev.set(); lg.join(); sampler.stop()
        end = rows[-1][0]
        _, oom1, _, _ = gexec("dmesg 2>/dev/null | grep -ci 'out of memory\\|oom-kill' || true")
        with open(os.path.join(out, f"concurrent-{name}-requests.csv"), "w", newline="") as f:
            w = csv.writer(f); w.writerow(["t_s", "latency_ms", "status"]); w.writerows(rows)
        with open(os.path.join(out, f"concurrent-{name}-build.log"), "w") as f:
            f.write(r["log"])
        results[name] = {"build_rc": r["rc"], "build_wall_s": r["wall_s"],
                         "before": phase_stats(rows, 0, b0), "during": phase_stats(rows, b0, b1),
                         "after": phase_stats(rows, b1, end),
                         "oom_lines_before_after": [oom0.strip(), oom1.strip()]}
        time.sleep(5)
    return results


def m_publish(out, n):
    stage_fixtures()
    docker("build", "-q", "--label", LABEL, "-t", "harpoon-baseline/fx-web:pub", os.path.join(FX_HOST, "fx-web"))
    rows, rnd = [], random.Random(7)
    for i in range(n):
        time.sleep(rnd.uniform(0, 10))  # sample the port-forwarder's 10 s timer phase
        port, name = 38100 + i, f"baseline-pub-{i}"
        t0 = now()
        rc, o, e, _ = docker("run", "-d", "--name", name, "--label", LABEL, "-p", f"{port}:8080",
                             "harpoon-baseline/fx-web:pub")
        t_run = now()
        ready = None
        while now() - t_run < 30:
            try:
                c = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                c.request("GET", "/index.html"); r = c.getresponse(); r.read(); c.close()
                if r.status == 200:
                    ready = now(); break
            except Exception:
                pass
            time.sleep(0.01)
        t_rm = now()
        docker("rm", "-f", name)
        closed = None
        while now() - t_rm < 30:
            try:
                socket.create_connection(("127.0.0.1", port), 0.2).close()
            except OSError:
                closed = now(); break
            time.sleep(0.01)
        rows.append({"i": i, "docker_run_s": t_run - t0,
                     "publish_lag_s": (ready - t_run) if ready else None,
                     "unpublish_lag_s": (closed - t_rm) if closed else None})
    with open(os.path.join(out, "publish.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, rows[0].keys()); w.writeheader(); w.writerows(rows)
    g = lambda k: [r[k] for r in rows if r[k] is not None]
    return {"docker_run_s": summ(g("docker_run_s")), "publish_lag_s": summ(g("publish_lag_s")),
            "unpublish_lag_s": summ(g("unpublish_lag_s")),
            "never_ready": sum(r["publish_lag_s"] is None for r in rows)}


def peak_rss_kb(argv, env=ENV):
    """Run argv under /usr/bin/time -l; return (wall_s, peak_rss_kB, rc)."""
    t0 = now()
    p = subprocess.run(["/usr/bin/time", "-l", *argv], capture_output=True, text=True, env=env)
    wall = now() - t0
    rss = 0
    for l in p.stderr.splitlines():
        if "maximum resident set size" in l:
            rss = int(l.split()[0]) // 1024  # macOS reports bytes
    return wall, rss, p.returncode


def m_idle(out, n):
    hp, vz = pids()
    sampler = Sampler(os.path.join(out, "idle-sampler.csv"), hp, vz); sampler.start()
    time.sleep(60)
    sampler.stop()
    res = {"note": "Deployment engine does not exist: engine idle cost is PENDING. Values below are proxies."}
    # proxy 1: one-shot Docker CLI invocations (what the engine would run per step)
    for label, argv in (("docker version", [DOCKER, "version", "--format", "{{.Server.Version}}"]),
                        ("docker ps", [DOCKER, "ps", "-q"]),
                        ("docker image inspect", [DOCKER, "image", "inspect", "alpine:3.22", "--format", "{{.Id}}"])):
        xs = [peak_rss_kb(argv) for _ in range(n)]
        res[f"proxy_oneshot:{label}"] = {"wall_s": summ([x[0] for x in xs]), "peak_rss_kB": summ([x[1] for x in xs])}
    # proxy 2: SQLite WAL write+fsync latency from a throwaway DB (Python sqlite3, NOT the Rust engine)
    import sqlite3, tempfile
    d = tempfile.mkdtemp(prefix="hb-sqlite-")
    c = sqlite3.connect(os.path.join(d, "t.db")); c.execute("pragma journal_mode=wal"); c.execute("pragma synchronous=full")
    c.execute("create table d(id integer primary key, state text, step text, at real)")
    lat = []
    for i in range(500):
        t = now(); c.execute("insert into d(state,step,at) values('running','build',?)", (time.time(),)); c.commit()
        lat.append((now() - t) * 1000)
    c.close(); shutil.rmtree(d, ignore_errors=True)
    res["proxy_sqlite_commit_ms"] = summ(lat)
    res["proxy_sqlite_python_self_peak_rss_kB"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss // 1024
    # proxy 3: one outbound poll (git ls-remote against the public repo)
    xs = []
    for _ in range(min(n, 10)):
        xs.append(peak_rss_kb(["git", "ls-remote", "https://github.com/krazybean/Harpoon.git", "HEAD"], env=os.environ.copy()))
        time.sleep(1)
    res["proxy_git_ls_remote"] = {"wall_s": summ([x[0] for x in xs]), "peak_rss_kB": summ([x[1] for x in xs])}
    return res


def m_transport(out, n):
    """Cost of the macOS->vsock->guest hop for a trivial API call, versus the same call inside the guest."""
    n = max(n, 30)
    host = [docker("version", "--format", "{{.Server.Version}}")[3] for _ in range(n)]
    ping = []
    import http.client as hc
    class U(hc.HTTPConnection):
        def connect(self):
            self.sock = socket.socket(socket.AF_UNIX); self.sock.connect("/tmp/harpoon-docker.sock")
    for _ in range(n):
        t = now(); c = U("localhost"); c.request("GET", "/_ping"); c.getresponse().read(); c.close(); ping.append((now() - t) * 1000)
    base = [sh([HARPOON, "exec", "--", "true"])[3] for _ in range(n)]
    guest = [sh([HARPOON, "exec", "--", "docker", "version", "--format", "{{.Server.Version}}"])[3] for _ in range(n)]
    return {"host_docker_cli_version_s": summ(host), "host_raw_api_ping_ms": summ(ping),
            "guest_exec_true_s": summ(base), "guest_docker_cli_version_s": summ(guest),
            "note": "guest CLI has no buildx: BuildKit builds cannot be run in-guest for a like-for-like build comparison"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["build", "concurrent", "publish", "idle", "transport", "cleanup", "env"])
    ap.add_argument("--out", default=os.path.join(ROOT, "..", "..", "docs", "results", "DEPLOY_BASELINE", "raw"))
    ap.add_argument("--n", type=int, default=5)
    ap.add_argument("--only", default=None)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    signal.signal(signal.SIGINT, lambda *x: (cleanup(), sys.exit(130)))
    if a.cmd == "cleanup":
        cleanup(); return
    if a.cmd == "env":
        print(json.dumps(env_info(), indent=2)); return
    fn = {"build": m_build, "concurrent": m_concurrent, "publish": m_publish, "idle": m_idle, "transport": m_transport}[a.cmd]
    try:
        res = fn(a.out, a.n, a.only) if a.cmd == "build" else fn(a.out, a.n)
    finally:
        cleanup()
    res["_env"] = env_info()
    name = a.cmd if not a.only else f"{a.cmd}-{a.only}"
    with open(os.path.join(a.out, f"{name}.json"), "w") as f:
        json.dump(res, f, indent=2, default=str)
    print(json.dumps(res, indent=2, default=str))


if __name__ == "__main__":
    main()
