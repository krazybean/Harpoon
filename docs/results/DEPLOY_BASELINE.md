# Deployment baseline measurements (Milestone B prerequisite)

Status: MEASURED on one host, one session — see [Uncertainty](#uncertainty-and-limits)
Date: 2026-10-30
Spec: [requirements — Baseline measurements](../deployment-control-plane-requirements.md)
Harness: [`tools/deploy-baseline/`](../../tools/deploy-baseline/) · raw data: [`DEPLOY_BASELINE/raw/`](DEPLOY_BASELINE/raw/)

This report separates **measured** values from **inference** and from **pending** items.
Nothing was invented for metrics that need an engine that does not exist.

## Summary and recommendation

**Recommendation: GO for Milestone B, with five conditions** (see
[Go / no-go](#go--no-go-for-milestone-b)). The measurements removed two risks the spec
carried and surfaced two it did not:

| Spec item | Result |
|---|---|
| 1. Dockerfile build time and memory through the socket | Measured. Socket hop costs ≈ 0.8 ms per API call; a 2-vCPU-saturating build takes 17.6 s ± 0.1 and lowers guest `MemAvailable` by ≈ 0.78 GiB for a 768 MiB working set (1.52 GiB for 1536 MiB). The like-for-like *in-guest* comparison **could not be run** (guest CLI has no buildx). |
| 2. Build while another app runs | Measured with a light app: **0 errors** in ~1,400 requests per run, no OOM, CPU saturated on both vCPUs. Memory near the 4 GiB limit was deliberately **not** tested. |
| 3. Loopback publish latency | Measured: reachable **0.64 s** median after `docker run` returns (max 0.69 s, n = 30). The feared 10 s port-forwarder delay is **not** the bound. |
| 4. Idle cost of the engine | **PENDING** (no engine). Proxies measured; the one-shot design implies no resident process. |
| New: `-p 127.0.0.1:PORT:…` breaks Harpoon forwarding | **Measured** (see §3). The engine must publish without a host IP. |
| New: BuildKit needs buildx on the host | Guest CLI lacks it (measured); the bundled macOS CLI relies on host plugins (code reading; clean-Mac behavior **not verified**). |

## Environment

| Item | Value |
|---|---|
| Host | Apple M3 Pro, macOS 27.0.1 (build 26A434), 18 GiB RAM |
| Harpoon runtime | `harpoon/build/harpoon run` (pid 67874, already running; not restarted), sha256 `33259856…f83a` |
| VM | 2 vCPU, 4096 MiB, kernel 6.12.110-0-virt, 32 GiB sparse disk |
| Docker (guest) | Engine 28.3.3 linux/arm64, overlay2, runc, cgroup v2, no swap |
| Docker (host CLI) | 29.9.0 with buildx v0.32.1-desktop.1 (plugins from the host's Docker Desktop install) |
| Starting state | 0 running containers, 9 stopped, 6 images, 1 volume, **build cache 0 B** |
| Host disk | 25 GiB free of 460 GiB at start |

Access is always through `DOCKER_HOST=unix:///tmp/harpoon-docker.sock`, never the
default Docker context (the host also has Docker Desktop contexts).

## Method

Disposable fixtures in `tools/deploy-baseline/fixtures/` (the Collectiv container was
**not** used; its database and volumes were not touched):

- `fx-web` — `alpine:3.22` (already local), 5 layers, a tiny `nc`-based HTTP responder.
- `fx-ctx` — `fx-web` plus a deterministic 50 MiB incompressible file (context upload).
- `fx-burn` — synthetic build load: two CPU-bound `sha256sum` workers (2000 MiB each) and
  an `awk` process holding 768 MiB (or 1536 MiB) of anonymous memory.

Every Docker object the harness creates carries the label `harpoon.baseline=1` and is
removed by label at exit. A sampler reads the guest (`/proc/meminfo`, `/proc/stat`,
`dockerd`/`containerd` RSS via `harpoon exec`) and the host (`ps` RSS and CPU time of the
Harpoon runtime and the Virtualization service) once per second. All commands:

```sh
cd tools/deploy-baseline
python3 bench.py env                 # environment
python3 bench.py build --n 5         # measurement 1
python3 bench.py transport --n 30    # socket-hop cost
python3 bench.py concurrent          # measurement 2
python3 bench.py publish --n 30      # measurement 3
python3 bench.py idle --n 20         # measurement 4 (proxies)
python3 bench.py cleanup             # removes only harpoon.baseline=1 objects
```

## 1. Dockerfile build time and memory through the Harpoon socket

Wall time of `docker build --no-cache` from macOS through `/tmp/harpoon-docker.sock`.

| Case | n | min | median | p95 | max | stdev |
|---|---|---|---|---|---|---|
| `fx-web` cold (`--no-cache`) | 5 | 1.18 s | **1.20 s** | 1.48 s | 1.54 s | 0.15 |
| `fx-web` warm (layer cache) | 5 | 0.35 s | **0.37 s** | 0.41 s | 0.42 s | 0.03 |
| `fx-ctx` cold (50 MiB context) | 3 | 1.39 s | **1.43 s** | 4.10 s | 4.40 s | 1.73 |
| `fx-burn` cold (CPU ×2 + 768 MiB) | 5 | 17.54 s | **17.63 s** | 17.73 s | 17.73 s | 0.10 |

- **Context upload.** The first upload of the 50 MiB context took 2.8 s (≈ 18.7 MB/s,
  from the BuildKit log `transferring context: 52.44MB 2.8s`); later builds transferred
  90 bytes because BuildKit syncs only changes. This is a *first-build* cost, n = 3, and
  cannot be split between the socket bridge and BuildKit.
- **Guest resources during `fx-burn`** (sampled at 1 Hz; see `raw/build-sampler.csv`):
  guest CPU reached **100 %** (both vCPUs); `MemAvailable` fell from ≈ 3.50 GiB at rest
  to a minimum of **2.72 GiB** for the 768 MiB allocation (−0.78 GiB) and **1.98 GiB** for
  1536 MiB (−1.52 GiB) in the concurrent runs; `dockerd` RSS stayed flat at ≈ 100 MiB and `containerd` at ≈ 47 MiB (BuildKit
  runs inside `dockerd`).
- **Host cost.** The Virtualization service RSS rose from ≈ 0.11–0.35 GB to a peak of
  **0.89 GB** (768 MiB) and **1.24 GB** (1536 MiB), and returned to 0.30–0.34 GB within the
  following 30 s. The Harpoon runtime process stayed at 13–15 MiB.
- **Socket hop cost** (`raw/transport.json`, n = 30): raw `GET /_ping` over the Unix
  socket median **0.81 ms** (p95 0.91 ms); host `docker version` median 16.5 ms; the same
  call inside the guest via `harpoon exec` median 73.6 ms, of which 67.1 ms is the
  `exec` round trip itself.
- **Not measurable: in-guest `docker build` comparison.** The guest's Docker CLI has no
  buildx; with `DOCKER_BUILDKIT=1` it refuses ("BuildKit is enabled but the buildx
  component is missing"), and the legacy builder is a different engine. Logs:
  `raw/build-guest-*.log`. The spec's "≤ 1.2× a direct `docker build`" criterion therefore
  has no *in-guest* baseline; the reference becomes the **host-CLI build through the
  socket** recorded above, and the engine-overhead comparison is **PENDING**.

## 2. Build behavior while another application runs

A published `fx-web` container served `GET /index.html` every 50 ms from the host
(≈ 20 req/s) for 30 s, then `fx-burn` was built, then 30 s more. Two runs; per-request
data in `raw/concurrent-*-requests.csv`.

| Run | Phase | Requests | Errors | p50 | p95 | p99 | max |
|---|---|---|---|---|---|---|---|
| mem 768 MiB | before | 524 | 0 | 5.1 ms | 9.4 ms | 16.3 ms | 32.0 ms |
| | during build (18.4 s) | 322 | 0 | 2.5 ms | 6.8 ms | 15.4 ms | 51.3 ms |
| | after | 525 | 0 | 4.9 ms | 9.1 ms | 18.5 ms | 34.0 ms |
| mem 1536 MiB | before | 523 | 0 | 5.6 ms | 9.5 ms | 16.3 ms | 20.1 ms |
| | during build (18.4 s) | 322 | 0 | 2.5 ms | 7.2 ms | 14.0 ms | 35.8 ms |
| | after | 523 | 0 | 4.7 ms | 9.3 ms | 25.6 ms | 41.2 ms |

- No guest OOM or `oom-kill` lines before or after either run (`dmesg` count 0 → 0).
- The build took 18.4 s with the app running versus 17.6 s alone (≈ +4 %).
- **Do not over-read the latency numbers.** The responder is trivial, so "no degradation"
  applies to a light app only. The *lower* median during the build is unexplained
  (hypothesis: host CPU wake-up effects on the request loop); it is reported as observed.
- **Not tested, on purpose:** a build that approaches the 4 GiB limit. Doing so could
  OOM-kill `dockerd` in the user's development VM. The largest validated working set is
  1536 MiB (≈ 38 % of VM memory). The failure threshold is **unknown**.

## 3. Localhost port-publishing latency

30 iterations: `docker run -d -p PORT:8080`, poll the host loopback port every 10 ms until
`HTTP 200`; then `docker rm -f` and time until the host port refuses. A random 0–10 s delay
before each iteration samples the forwarder's 10 s timer phase.

| Interval | n | min | median | p95 | max |
|---|---|---|---|---|---|
| `docker run -d` returns | 30 | 0.21 s | **0.25 s** | 0.36 s | 0.45 s |
| Reachable on host after it returns | 30 | 0.00 s | **0.64 s** | 0.67 s | 0.69 s |
| Port closed after `docker rm -f` | 30 | 0.19 s | **0.21 s** | 0.74 s | 0.92 s |

- 0 of 30 never became reachable. No sample exceeded 0.7 s: 18 of 30 fell between 0.62 and
  0.69 s and the other 12 were reachable sooner (0.00–0.58 s). The 10 s timer was never the
  bound, which suggests publishing is **event-driven (≈ 0.65 s)** with the poll as a
  fallback. (Inference from the distribution and the runtime log events
  `HARPOON_PORT_FORWARD_ADD` / `_LISTENING`; the dependence on timer phase was not
  analyzed.)
- **Pitfall found — host-IP-bound publishing does not work.** With
  `-p 127.0.0.1:PORT:8080` the host listener appears but every connection fails: the
  runtime log shows `HOST_FORWARD_CONNECT_FAILED … 192.168.64.3:PORT … Connection refused`.
  The forwarder connects to the guest's NAT address, and Docker bound the port to the
  guest's loopback only. The same container is reachable from inside the guest. Publishing
  as `-p PORT:8080` works, and Harpoon exposes it on host loopback. Consequence: the
  engine must not pass a host IP to `-p` (and Compose `ports: "127.0.0.1:…"` has the same
  problem). Reproduced three times with different servers (`raw` evidence is the runtime
  log; the failing runs were an `nc` HTTP responder, an echo server and a banner server).

## 4. Idle CPU and memory cost of the deployment engine

**Engine idle cost: PENDING.** The engine does not exist. Everything below is a proxy and
must not be read as an engine measurement.

Harpoon runtime at idle (60 s, no containers, 1 Hz sampling — the sampler's own
`harpoon exec` per second inflates the CPU figures, by an amount not isolated):

| Component | RSS (median / range) | CPU (median / max) |
|---|---|---|
| Harpoon runtime process | 14.4 MiB (12.8–15.3) | 3.0 % / 5 % (observer-inflated) |
| Virtualization service | 140 MiB (96–249) | 7.9 % / 13 % (observer-inflated) |
| Guest `dockerd` / `containerd` | 100 / 47 MiB | — |
| Guest `MemAvailable` | 3.53 GiB of 3.83 GiB | guest CPU busy median 3.8 % |

Proxies for what a one-shot engine would do (n = 20 unless noted):

| Proxy | Wall median (p95) | Peak RSS median |
|---|---|---|
| host `docker version` / `docker ps` / `docker image inspect` | 21 / 16 / 17 ms (42 / 18 / 20 ms) | 28.4 / 28.4 / 28.4 MiB |
| `git ls-remote` against the public Harpoon repo (outbound poll), n = 10 | 0.30 s (0.40 s) | 11.8 MiB |
| SQLite commit, WAL, `synchronous=FULL` (Python, throwaway DB, n = 500) | 0.048 ms (0.157 ms) | Python interpreter 25.2 MiB — **not** representative of Rust |

- **Caveat for crash-safety claims.** The sub-0.1 ms SQLite commit time shows macOS did
  not flush the disk cache; SQLite only does so with `PRAGMA fullfsync=ON`. Latency from
  this proxy says nothing about power-loss durability. The engine must choose and measure
  `fullfsync` explicitly.
- The polling proxy implies a duty cycle of ≈ 0.5 % (0.30 s per 60 s) and ≈ 12 MiB
  transient RSS for a 60 s poll — an estimate, not a measurement of the engine.

## Recommended resource budgets (to adopt in Milestone B, to be re-measured)

| Budget | Value | Basis |
|---|---|---|
| Engine resident process at idle | **none** (one-shot CLI); any `watch` process ≤ 30 MiB RSS and ≤ 0.5 % CPU | spec stop condition; poll proxy 12 MiB / 0.5 % |
| Engine transient memory per operation | ≤ 64 MiB engine + ≈ 28 MiB per `docker` child | CLI proxy 28.4 MiB; engine **PENDING** |
| Per Docker call overhead | ≈ 20 ms (CLI) / ≈ 1 ms (raw API) | measured |
| Concurrent builds | **1 globally** | CPU saturates both vCPUs; +4 % wall for a light co-resident app |
| Validated build working set | ≤ 1.5 GiB on the default 4 GiB VM | largest tested; failure threshold unknown |
| Host memory headroom for a build | ≈ +1 GB above idle Virtualization RSS | peak ≈ 1.2 GB at 1536 MiB, from ≈ 0.1–0.35 GB at rest |
| Replacement visibility (publish) | plan for ≈ 1 s after `docker run` | publish median 0.64 s, max 0.69 s |
| Cutover interruption | **PENDING** — `docker stop` grace period, app start and health probe were not measured | the UI mock value of "about 4 s" is illustrative only |

## Resource and disk side effects of this run

- All containers and images created by the harness were removed (0 labelled objects
  remain). **121 BuildKit cache records (367 MB) remain** because cache pruning was out of
  scope and the cache was empty before the run (build cache 0 B), so they are all from
  this harness. They can be removed with `docker buildx prune` if wanted; this report
  does not do it.
- The VM disk image's host allocation grew from 1.18 GB to 1.55 GB, which is consistent with
  the retained cache plus layers; a deployment engine therefore needs a retention policy
  for build cache, which the spec currently defers.

## Go / no-go for Milestone B

**GO**, because no measurement contradicts the approved architecture (host-side engine
over the Docker socket, one build at a time, polling triggers, loopback publishing), and
the previously unknown costs are small. Conditions to carry into Milestone B:

1. **Publish without a host IP** (`-p PORT:CPORT`); add a failing-then-passing test for
   `127.0.0.1` binding and document the Compose pitfall.
2. **Decide BuildKit availability.** Guest has no buildx; confirm the host CLI the engine
   will use has buildx on a clean Mac (not verified here), or require it for Milestone C
   BuildKit secrets.
3. **Measure the cutover** on a real replacement: `docker stop` grace period, app start,
   preliminary and final checks. This is the number the spec asks the UI to display.
4. **Choose SQLite durability** (`fullfsync`) and measure commit cost with it.
5. **Define build-cache retention** and a free-memory pre-check before building, since
   the OOM threshold is untested.

Still **PENDING** (cannot be measured yet): engine idle CPU/RSS; engine overhead versus a
direct build (the ≤ 1.2× criterion); `watch` process cost; probe mechanism comparison
(`docker exec` vs published port — the published-port probe is viable at ≈ 1 s).

## Uncertainty and limits

- One Mac, one session, small samples (n = 3–30); percentiles above n = 10 are indicative.
- Synthetic fixtures; a real Node/Python build (npm/pip, network, larger layers) will have
  different CPU/memory/disk behavior. The existing Collectiv workload was not used.
- The 1 Hz sampler runs `harpoon exec` once per second, which adds guest and host CPU
  (not isolated); memory figures are less affected.
- The VM was already running for 2+ hours with warm caches; a cold start was not measured.
- The load generator and the harness share the host with other work on the machine.
- Alpine's `busybox` has no `httpd` applet; the fixture server uses `nc -e`, a simple
  responder that is not representative of a production web server.
- Only Apple Silicon was measured. Intel/universal builds and a second Mac are not covered.
