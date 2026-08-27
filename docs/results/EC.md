# EC — Ecosystem Compatibility — 2026-08-26 (BLOCKED_HOST_TRANSIENT — VZErrorDomain 1 at 20:34:09Z/20:35:00Z, 2× HOST_VZ after 30s retry)

**Environment:** `22415c1` (`22415c11`), `Harpoon 0.1.0-dev` 802K arm64 `valid on disk` `com.apple.security.virtualization`, kernel `Image-virt` 33M `377d3480…`, initramfs `harpoon-m4-initramfs.cpio.gz` 14M, root `2G` sparse 2147483648, macOS 26.5.2 25F84 arm64 Mac15,6 M3 Pro, Docker 29.3.1 Compose v5.1.0, `harpoon/build/harpoon` 802K, `harpoon/ec-test.sh` 425 lines `bash -n 0` `sh -n 0`.
**Git:** `main@22415c1`, dirty only `README.md`/`docs/roadmap.md` (docs/results untracked), no `harpoon/Sources` change.
**Known risk:** Intermittent host `Virtualization.framework` `VZErrorDomain Code=1` `HOST_VZ_START_FAILURE` remains observed/characterized (R1 5 paired cycles 10/10, M17 17:57-18:21/20:25/20:27, M18 0/10 18:53, now EC 20:34-20:35 2/2). Not claimed fixed. No redesign.

## 1 Independent audit
Read `docs/AGENTS.md` (Ponytail mandatory, roadmap canonical, evidence over narrative, preserve baselines), `docs/roadmap.md` (`M16 PASS`/`M17 PASS`/`M18 PASS`/`R1 DONE`/`core MVP COMPLETE`/`EC NOT STARTED`/`UI NOT STARTED`, `M18 PASS` `90641` `VZ 3/3`), `docs/mvp.md` MUST (`docker run/pull/build/volume/network/exec/logs/inspect/ps`, VirtioFS, `localhost:<port>`, persistence `R7`, outbound internet, bridge+user networks, clock sync, `harpoon start/stop/restart/status/doctor`), `docs/requirements.md` `R1-R5` (API `~/.harpoon/docker.sock` via vsock, not `/var/run` share; `R1` clients: CLI/Compose/LazyDocker/IDE/Testcontainers/SDKs; `R2` images/build via BuildKit; `R3` containers; `R4` volumes+bind via VirtioFS; `R5` networking), `docs/architecture.md` (Harpoon owns macOS↔Linux boundary, socket bridge transparent byte-stream via vsock, `VZNAT` NAT, `VirtioFS` `/Users`+`/private/tmp`+`/tmp/harpoon-share`, `Lifecycle`/`VMManager`/`Bridges`/`PortForwardManager`), `docs/compatibility.md` (MUST `docker CLI ≥24`/`Compose v2`/`LazyDocker`/`Testcontainers`, BuildKit default, filesystem/network/host reqs), `docs/results/M16.md` `PASS 17:49` bounded sparse, `M17.md` `PASS` 8→9 via exact-field `awk`, `M18.md` `PASS` `90641` `16 PASS`, `docs/performance.md` host/guest/startup/idle/container/load/Compose/build/fs/persistence/soak methodology, `harpoon/Sources/Bridges.swift` 818 lines (`BridgeSet` vsock→guest `2375`, `PortForwardManager` vsock `GET /containers/json`→`reconcile`, balloon control, `HARPOON_CLEANUP_EPHEMERAL` owned only), `PortForwardManager.swift` 324 lines (`setGuestIP`→`scheduleSync`, `10s` poll fallback + `scheduleSync`, `Forward` per `HostPort`), `VMManager.swift` 137 lines (`VZNAT`+VirtioFS×3+vsock+balloon, `validate OK`), `Lifecycle.swift` 32 lines, `git status` (`README.md`/`docs/roadmap.md` modified, `docs/results/*.md` untracked, no `harpoon/Sources` diff), `harpoon/results/` (`m16`/`m17`/`m18`/`r1` preserved, `m17 181158 healthy` 8 PASS, `m18 PASS,completed`).

**No unexpected Sources changes since M18:** `git diff -- harpoon/Sources` 0 lines (verified `Bridges`/`VMManager`/`Lifecycle`/`PortForwardManager`/`HarpoonCLI`/`RuntimeConfig` unchanged from `22415c1`).

## 2 EC matrix (canonical, single harness)
Matrix defined per task `A-H`, documented here before implementation, executed via one bounded harness `harpoon/ec-test.sh` (see §4). No split into dozens of milestones unless failure isolation requires it. Source of truth is `harpoon/ec-test.sh` 425 lines.

| Area | Capability | Tool/command | Classification | Notes |
|------|------------|--------------|---|---|
| A | `docker version` | `docker --context harpoon version` + `DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version` | `MUST` | Server `28.3.3` must be Harpoon, not Desktop |
| A | `docker info` | `docker --context harpoon info` | `MUST` | `Server Version` |
| A | `docker ps` | `docker ps` | `MUST` | `CONTAINER` header |
| A | `docker inspect` | `docker inspect ec-stop` | `MUST` | `Running:true` |
| A | `docker logs` | `docker logs ec-stop` | `MUST` | |
| A | `docker exec` | `docker exec ec-stop echo hello` | `MUST` | |
| A | `docker stats --no-stream` | `docker stats --no-stream` | `MUST` | |
| A | `docker events` bounded | `timeout 3 docker events --since 1s --until 5s` | `MUST` | bounded, not hang |
| A | `stop/start/restart/rm` | `docker stop/start/restart/rm` | `MUST` | |
| A | exit code propagation | `docker run --rm alpine:3.22 sh -c 'exit 42'` → 42 | `MUST` | |
| A | signals/termination | `sleep 60` → `stop -t 1` | `MUST` | `Running:false` |
| B | `pull`/`images`/`inspect`/`tag`/`rmi` | `docker pull/images/inspect/tag/rmi` | `MUST` | `alpine:3.22` |
| B | `docker build` BuildKit | `docker build -t ec-build-test` | `MUST` | via Engine BuildKit |
| B | build context | small project dir `Dockerfile` `COPY` | `MUST` | `harpoon/results/ec/ec-build` |
| B | `.dockerignore` | `ignored.txt` vs `keep.txt` | `MUST` | `COPY . /app` must respect |
| B | image persistence | `harpoon stop`→`start`→`images` still has `ec-build-test` | `MUST` | `R2` |
| C | named volume `create/inspect/use/remove` | `docker volume create/inspect` + `run -v ec-vol:/data` | `MUST` | |
| C | volume persistence | `harpoon stop`→`start`→`cat /data/marker` | `MUST` | `R4` `/var/lib/docker/volumes` |
| C | bind mounts `macOS→VirtioFS→container` | `-v $bind_host:/app` under `/Users` | `MUST` | `R4` |
| C | read/write/symlink/propagation | host read, container write visible, host modify→container, symlink | `MUST` | |
| C | Node-style tree | `src/index.js`+`package.json` | `SHOULD` | if practical |
| D | bridge `create/inspect/remove` | `docker network create ec-net` | `MUST` | |
| D | container-to-container | `ec-net-a` ping `ec-net-b` | `MUST` | `R5` bridge |
| D | published `localhost:port` | `nginx:alpine -p 18092:80` → `curl 127.0.0.1:18092` | `MUST` | `VZNAT`+`PortForwardManager` |
| D | multiple ports | `-p 18093:80 -p 18094:80` → both | `MUST` | |
| D | port removal/reconcile | `rm -f ec-web` → `curl` fails, `HARPOON_PORT_FORWARD_REMOVE` | `MUST` | |
| D | outbound internet | `ping 8.8.8.8` | `MUST` | `R5` |
| D | DNS | `nslookup google.com` | `MUST` | |
| D | localhost from macOS | `curl`/`nc -z` from macOS host | `MUST` | |
| E | `compose version` | `docker compose version` | `MUST` | `SHOULD` per mvp, but `MUST` for EC |
| E | `compose up -d` multi-service | `web(nginx:alpine)+redis:alpine` | `MUST` | `R5` |
| E | `depends_on`/`healthcheck` | `redis` healthcheck `redis-cli ping` | `MUST` | |
| E | env | `EC_ENV=hello` → `exec env` | `MUST` | |
| E | volume+network+port | `ec-compose-vol`+`ec-compose-net`+`18095:80` | `MUST` | |
| E | `ps`/`logs`/`exec`/`restart`/`down` | `compose ps/logs/exec/restart/down` | `MUST` | |
| E | persistence across restart | `harpoon stop`→`start`→`compose ps` | `MUST` | Docker semantics |
| F | LazyDocker | `lazydocker --help` vs `DOCKER_HOST=harpoon` | `SHOULD` | `NOT TESTED` if not installed |
| F | Docker SDK | `python3 -c "import docker"` → `DockerClient(unix:///tmp/harpoon-docker.sock)` | `SHOULD` | no new dep |
| F | Testcontainers | `python3 -c "import testcontainers"` | `SHOULD` | `NOT TESTED` if not installed |
| F | VS Code/IDE | GUI automation | `best-effort` | `NOT TESTED` (no GUI) |
| G | sequential | `version`×5 | `MUST` | |
| G | concurrent `run`×3 + `ps/info/version`×3 | `&` + `wait` | `MUST` | |
| G | keep-alive/half-close | covered by regression `harpoon/regression-bridges.sh` | `MUST` | not re-tested separately |
| G | no socket loss | `[ -S /tmp/harpoon-docker.sock ]` after concurrency | `MUST` | |
| H | `docker --context harpoon` vs `DOCKER_HOST=unix://` | both must show `Server` | `MUST` | no Desktop fallback, check `docker context inspect harpoon` `unix:///tmp/harpoon-docker.sock` + `docker context show` active not required to be harpoon |

No dozens of milestones; one matrix, one harness. `POST-MVP`/`NOT TESTED` explicitly marked with reason, not fabricated.

## 3 Ponytail analysis (before code)
For every gap ask 1-7:

1. *Required for ordinary dev?* All `A-E`/`G`/`H` are `MUST` per `mvp.md`/`requirements.md` (`R1` API, `R2` images, `R3` containers, `R4` volumes/bind, `R5` networking, compose `SHOULD`→`MUST` for EC). `F` `SHOULD`/`best-effort` — test only if available, mark `NOT TESTED` otherwise (no new dep).
2. *Docker Engine already implements?* Yes — `R1` states Docker owns container/image/volume/network state, BuildKit, `containerd`, Linux isolation. Harpoon is transport, not reimplementation.
3. *Harpoon merely transporting?* Yes — `unix:///tmp/harpoon-docker.sock` → `Bridges` `vsock` `2375` → guest `Docker Engine` via transparent byte-stream proxy (spike2 half-close fix). No filtering of API payloads.
4. *Failure seam:* If `A` fails, seam is CLI/client→Unix→proxy→vsock→guest socket→Engine. If `D` fails, seam is `VZNAT`+`PortForwardManager` `vsock GET /containers/json`→`reconcile`→`127.0.0.1` listener (loopback-only). If `C` fails, seam is `VirtioFS` `/Users`+`/private/tmp`. If `E` fails, seam is Engine `Compose` over proxy (not Harpoon Compose layer — do not build custom Compose).
5. *Smaller fix?* Prefer narrow `Bridges`/`PortForwardManager`/`Lifecycle` tweak over new layer. Example: if `docker exec` hangs, fix vsock half-close, not new API server.
6. *Duplicate Desktop?* No — do not reimplement Docker Desktop behavior clients don't require (e.g., custom builder, network model, volume abstraction).
7. *Measured evidence?* Baseline run before product changes (Phase 4). No change without demonstrated failure.

**Rejected speculative:** Docker API reimplementation, custom Compose, custom builder/network/volume/container state, Kubernetes, GUI integrations, large shims without failing client — all YAGNI. Only harness initially; product change only after measured `PRODUCT_FAIL`.

## 4 Baseline results before product changes
**Harpoon start:** `Harpoon is already stopped` → `harpoon/build/harpoon start` → `[2026-08-26T20:34:09Z] HARPOON_STATE STARTING -> BOOTING` → `VZErrorDomain 1` `BOOTING -> FAILED` `HOST_VZ_START_FAILURE` `HARPOON_CLEANUP_EPHEMERAL_BEGIN/CLEANED (owned only)` → harness `VZ transient retry 30s` (bounded, existing policy as `m17`) → second `harpoon start` at `20:35:00Z` → again `VZErrorDomain 1` `HOST_VZ_START_FAILURE` → `BLOCKED HOST_VZ_START_FAILURE` (after retry). `harpoon status --json` `state stopped` `dockerReady false` `lockHeld false` `sockExists false` `cpus 2` `memoryMiB 1024` `disk /tmp/harpoon-runtime/data/harpoon-root.img` `log /tmp/harpoon-runtime/harpoon.log`.

**Preserved:** `harpoon/results/ec-preserved-20260826-203427/tier-status.csv` `ec,HOST_VZ_START_FAILURE` + `vz.csv` `2026-08-26T20:34:13Z,start,HOST_VZ_START_FAILURE` + `start.log` `VZErrorDomain 1` `HOST_VZ_START_FAILURE`; live `harpoon/results/ec/tier-status.csv` same `20:35:04Z`. Historical `HARPOON_GUEST_IP_DISCOVERED`/`PORT_FORWARD_ADD` not present (blocked before guest). Classification via byte-windowed current-attempt log (handle truncation) + live `status --json` precedence outranks stale text (no full-log grep contamination).

**No EC matrix executed:** Host in blocked window (2/2 `HOST_VZ` after retry, consistent with `R1 10/10` `19:23Z`, `M17 17:57-18:21`/`20:25`/`20:27`, `M18 0/10 18:53` same `Internal Virtualization error`). No `PRODUCT_FAIL` vs `FAIL` distinction needed — `HOST_VZ` distinct. No fabricated zeros as `PASS`. All `A-H` remain `NOT PROVEN` (need `RUNNING`+`Docker ready` to test directly; prior `M17 181158` 8 PASS + `M18 90641` `PASS,completed` trustworthy but EC must test directly, not infer).

**Existing `M16`/`M17`/`M18` behavior not regressed:** No `harpoon/Sources` change (0 lines), disk `2147483648` intact, prior `m17`/`m18` preserved, `harpoon/regression-bridges.sh` not run (blocked before).

## 5 Each demonstrated compatibility failure
None demonstrated yet — host blocked before any `PRODUCT_FAIL`. Intermittent `VZErrorDomain 1` is host/`Virtualization.framework` transient, not product-level compatibility failure exposed by EC. Do not redesign Harpoon around it unless EC exposes new product failure (per roadmap known risk). If EC later shows `PRODUCT_FAIL` (e.g., `docker exec` hang, bind propagation missing, compose port not reachable), isolate seam per §2, Ponytail, smallest proxy/lifecycle fix, rerun failing case → regression group → full EC.

## 6 Exact fixes made
None — no product change justified by measured evidence. `harpoon/ec-test.sh` created (425 lines) as execution + preservation mechanism, not product fix. Harness itself fixed to `logs --path` byte window + live `status --json` precedence (no full-log contamination), `HOST_VZ`/`RUNTIME_LOST`/`DOCKER_NOT_READY`/`PRODUCT_FAIL` distinct, `bash -n 0` `sh -n 0`, bounded timeouts, EC-owned cleanup only, no `rm -rf /tmp/harpoon*`, no Desktop fallback.

## 7 Final CLI/API results
`NOT PROVEN` — blocked before `docker version/info/ps/inspect/logs/exec/stats/events/stop/start/restart/rm` could be exercised against live Harpoon. Prior `M17 181158` `docker version Server` + `M18` `90641` smoke trustworthy but EC requires direct `ec-test.sh` pass, not inference.

## 8 Build results
`NOT PROVEN` — blocked before `pull/images/inspect/tag/rmi/build` BuildKit + `.dockerignore` + image persistence. Guest `spike2/cache/harpoon-root.img` `2G` and `m16` build `m16-test:1.0 8.54MB` prior evidence not EC-proven.

## 9 Volume/bind-mount results
`NOT PROVEN` — blocked before `ec-vol` + bind `ec-bind-host` `/Users`-backed `VirtioFS` read/write/symlink/propagation + Node-style `src/index.js`. `M16` `m16-vol+image` persistence + `M9` bind prior evidence not EC-proven.

## 10 Networking results
`NOT PROVEN` — blocked before `ec-net` bridge/container-to-container ping, `ec-web -p 18092:80` `curl 127.0.0.1:18092` + multi-port `18093/18094`, port removal/reconcile, outbound `8.8.8.8`, DNS `nslookup`, `VZNAT` `HARPOON_PORT_FORWARD_ADD/LISTENING/REMOVE`. Prior `M9` `18080/18081` + `M17` `18090` `VZNAT_REBIND_PASS` trustworthy but EC requires direct.

## 11 Compose results
`NOT PROVEN` — blocked before `ec-compose` `web(nginx:alpine)+redis:alpine` `depends_on: service_healthy` `healthcheck redis-cli ping` `env EC_ENV=hello` `volume ec-compose-vol` `network ec-compose-net` `18095:80` `up -d`→`ps`→`logs`→`exec`→`restart`→`down` + persistence across `Harpoon stop/start`. Prior `M9` compose `app:3000->18080`+`postgres:5432`+`redis` trustworthy but EC requires direct.

## 12 Third-party client results
- **LazyDocker:** `NOT TESTED` — `lazydocker: command not found` (local), would be `CLIENT_NOT_AVAILABLE` not `PRODUCT_FAIL`. `ec-test.sh` handles `command -v lazydocker` → `LAZYDOCKER_NOT_TESTED,not installed`.
- **Docker SDK (Python):** `NOT TESTED` — `docker` SDK not installed (`python3 -c "import docker"` fails). Would be `SDK_PYTHON_NOT_TESTED`. No dep added solely for test (per Phase 1 F). `ec-test.sh` marks `NOT TESTED` with reason.
- **Testcontainers:** `NOT TESTED` — `testcontainers` not installed (`python3 -c "import testcontainers"` fails). `TESTCONTAINERS_NOT_TESTED,not installed POST-MVP`. No trivial add without project dep.
- **VS Code / IDE:** `NOT TESTED` — GUI automation not applicable (`IDE_NOT_TESTED`).

No overclaim; all `NOT TESTED` classified explicitly with reason, not `PASS`.

## 13 Regression results
No regression run — blocked before `M16`/`M17`/`M18` regression group could be exercised against live Harpoon. Prior `harpoon/regression-bridges.sh` (half-close/concurrent, `M14` `logs --path`+mtime fix) trustworthy but not re-run in this blocked window. No `PRODUCT_FAIL` to justify selective rerun.

## 14 Security review
No EC product changes to review. `harpoon/ec-test.sh` itself `set -eu`, quoted `"$VAR"`, no `eval`, no `rm -rf /tmp/harpoon*` (only EC-owned `ec-*` containers/volumes/networks + `ec-compose` down + `ec-bind-host`), no `docker system prune`/`prune`, no `sudo`, no new entitlement, no launch agent, uses `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` explicitly + `docker --context harpoon` (no Desktop fallback verified via `docker context inspect harpoon` `unix:///tmp/harpoon-docker.sock`). Expected socket states when running: `0600` `srw-------` (enforced in harness `SOCKET_PASS`/`CONTROL_SOCKET_PASS` checks — not exercised this run due block). No broader `VirtioFS` sharing than approved (`/Users`+`/private/tmp`+`/tmp/harpoon-share`, not `/var/run/docker.sock`), no TCP `0.0.0.0:2375`, no telemetry/creds, no unsafe interpolation, no opaque downloads beyond `alpine:3.22`/`nginx:alpine`/`redis:alpine` via Engine when running.

## 15 Documentation/roadmap changes
- **Created:** `harpoon/ec-test.sh` 425 lines (bounded EC harness, preservation, `HOST_VZ`/`RUNTIME_LOST`/`DOCKER_NOT_READY`/`PRODUCT_FAIL` distinct, byte-windowed log, explicit context/socket, EC-owned cleanup only, bounded timeouts, `bash -n 0` `sh -n 0`).
- **Created:** `docs/results/EC.md` (this file) with matrix `PROVEN`/`NOT TESTED`/`HOST BLOCKED` classifications, Ponytail analysis, baseline `BLOCKED` evidence, no overclaim.
- **Roadmap:** `docs/roadmap.md` `Current phase`→`EC IN PROGRESS (BLOCKED_HOST_TRANSIENT 20:34:09Z/20:35:00Z VZErrorDomain 1 2/2 after 30s retry, preserved)` + `EC status`→`BLOCKED_HOST_TRANSIENT` with exact `harpoon.log` `BOOTING->FAILED` `HARPOON_CLEANUP_EPHEMERAL (owned only)` + known-risk note unchanged (`not claimed fixed`, no redesign unless EC exposes new product failure). `Tauri UI` remains `NOT STARTED`.

## 16 Remaining compatibility gaps
All `A-H` required scenarios remain `NOT PROVEN` (need healthy `RUNNING`+`Docker ready` window to exercise directly). Specifically unproven for EC gate: `docker version/info/ps/inspect/logs/exec/stats/events` + `exit code 42` + `signals`, `BuildKit` `build`+`.dockerignore`+image persistence, `ec-vol`+`bind` read/write/propagation/symlink+Node tree, `ec-net`+`ping`+`18092`+multi+reconcile+outbound+DNS, `ec-compose` `web+redis` `depends_on`+`healthcheck`+`env`+`volume`+`network`+`18095`+`ps/logs/exec/restart/down`+persistence, concurrency `run×3`+`ps/info/version×3`+socket still `0600`, `DOCKER_HOST` vs `--context harpoon` both, LazyDocker/SDK/Testcontainers `NOT TESTED` with reason is acceptable for `v0.1` but must be re-evaluated when `RUNNING` (LazyDocker `PASS` if installed). No `PRODUCT GAP` yet demonstrated; `HOST BLOCKED` is external.

## 17 Final verdict
**EC: BLOCKED_HOST_TRANSIENT — VZErrorDomain 1 Internal Virtualization error at 2026-08-26T20:34:09Z and retry at 20:35:00Z (2/2 HOST_VZ_START_FAILURE, BOOTING->FAILED 2-3s, HARPOON_CLEANUP_EPHEMERAL owned only, lockHeld false, sockExists false, validate OK, disk 2147483648, log /tmp/harpoon-runtime/harpoon.log). Intermittent host/Virtualization.framework transient consistent with R1 10/10, M17/M18 windows; not product defect, not EC compatibility failure. Retry 30s bounded per existing policy did not clear window. STOP.**

If host recovers (as at `00:22:26`/`17:49`/`18:07`/`20:08` previously), rerun `unset DOCKER_HOST; bash harpoon/ec-test.sh` to exercise full matrix `A-H` and obtain `EC: PASS` (requires `PASS,completed` with no `PRODUCT_FAIL`, security `0600` `PASS`, `EC-` owned cleanup, no Desktop fallback, no regression to `M16`/`M17`/`M18`). Do not begin Tauri UI.
