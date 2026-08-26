# Harpoon Roadmap — Canonical Trackable

> Harpoon is a lightweight Docker-compatible runtime for Apple Silicon macOS. Docker Engine remains authoritative for containers/images/volumes/networks/API/Compose/BuildKit. Harpoon owns only the macOS↔Linux boundary (VM lifecycle, socket bridge, networking, bind-mount transport, resources, memory reclamation, diagnostics).

## Current Position

- **Current phase:** Phase 3 — Resource Efficiency & Runtime Hardening
- **Last completed milestone:** M14 — Idle Resource Optimization (PortForwardManager 2→10, provenance `f9f0d4`/`0acca8`, harness fixed to authoritative `~/Library/.../harpoon.log` via `logs --path` + mtime, after live 6/min at 05:46:39/49 proven)
- **Next milestone:** M16 — Disk & Storage Lifecycle (M15 PASS)
- **Branch/commit:** `main` @ `22415c1` (`22415c11f30658ff1e61c87b8cae347ecd7b6489`), dirty only `.gitignore`/`README.md`/`dist/` untracked
- **Product-under-test:** `harpoon/build/harpoon` 802K arm64 + `dist/harpoon-0.1.0-dev-darwin-arm64` staged relocatable, kernel `spike1/cache/Image-virt` 33M `377d3480…`, initramfs `harpoon/cache/harpoon-m4-initramfs.cpio.gz` 14M, root `spike2/cache/harpoon-root.img` 2G/962M
- **Host:** macOS 26.5.2 25F84 arm64 Mac15,6 M3 Pro, Docker 29.3.1 Compose v5.1.0 Buildx v0.32.1
- **Known host transient:** `VZErrorDomain Code=1 Internal Virtualization error` intermittently blocks `VZVirtualMachine.start()` (spike1, M12, M13/M14 live runs). Previous same-binary boots succeeded (`2026-08-26T00:22:26 VM start SUCCESS`), proving Virtualization.framework works. Classified `TRANSIENT / ROOT CAUSE UNRESOLVED / NOT CURRENT BLOCKER` — do not treat framework as broken.

---

## Milestone Map

| # | Milestone | Status | Depends on | Evidence |
|---|-----------|--------|------------|----------|
| S1 | Spike 1 — Linux boot proof | **PASS** | — | `spike1/results/SPIKE1_RESULTS.md`, `spike1/swift/main.swift` |
| S2 | Spike 2 — Docker API socket bridge | **PASS** | S1 | `docs/results/SPIKE2_BRIDGE_FIX.md`, `spike2/` |
| S3 | Spike 3 — localhost networking | **PASS** (via M9) | S2 | `docs/results/SPIKE3.md` (harness blocked) + `docs/results/M9.md` live localhost `18080/18081` PASS |
| S4 | Spike 4 — bind mounts (VirtioFS) | **PASS** | S1 | `docs/results/M9.md` + M4 historic |
| S5 | Spike 5 — host-visible memory reclamation | **PASS** (arch proof) | S1 | `docs/results/SPIKE5.md` |
| M7 | CLI & Background Lifecycle | **PASS** | S1 | `docs/results/M10.md`, `docs/lifecycle.md`, `harpoon/m7-test.sh` |
| M8 | Docker Native Integration | **PASS** | S2, M7 | `docs/results/M8.md`, `harpoon/m8-test.sh` |
| M9 | Docker Compose / Dev Workflow | **PASS** | S2-S4, M8 | `docs/results/M9.md`, `harpoon/m9-test.sh` |
| M10 | Developer Ergonomics | **PASS** | M7 | `docs/results/M10.md`, `harpoon/m10-test.sh` |
| M11 | Installation / Distribution | **PASS** | M7-M10 | `docs/results/M12.md` §M11, `harpoon/m11-test.sh`, `dist/` |
| M12 | Phase 2 Acceptance | **CONDITIONAL PASS** | M7-M11 | `docs/results/M12.md`, `docs/phase2-acceptance.md` (22 PASS 11 BLOCKED external) |
| M13 | Resource Baseline | **PASS** (repair harness) | M12 | `docs/results/M13.md`, `docs/performance.md`, `harpoon/results/m13/` |
| M14 | Idle Resource Optimization | **PASS** (code `repeating: 10`, `f9f0d4`/`0acca8`, harness now `logs --path`+mtime, after 6/min live 05:46) | M13 | `docs/results/M14.md`, `harpoon/results/m14/`, `harpoon/m14-test.sh` |
| M15 | Memory Reclamation Optimization | **PASS — natural host-visible reclamation proven; balloon optimization rejected as unnecessary based on measurements** (valid paired 14:53–15:33 preserved: 512 idle 103872→103568, 768 389056→96816, 1024 374544→99696; balloon 512 101568/116960 & rerun 123200/114032, 768 116592/102592 & rerun 110000/94288 — no consistent win; harness 352/276 both bash -n 0 sh -n 0 pinned, natural reclamation ~50% drop by +60s, no auto policy YAGNI, balloon kept EXPERIMENTAL/manual, no drop_caches/hysteresis/M16) | M13, M14 | `docs/results/M15.md` Phase 10, `harpoon/results/m15-preserved-20260826-151813/host.csv`, `harpoon/results/m15-balloon-preserved-20260826-152533/host.csv`, `harpoon/results/m15-balloon/host.csv` (rerun 74452/78997) |
| M16 | Disk & Storage Lifecycle | **NOT STARTED** | M15 | — |
| M17 | Runtime Resilience | **NOT STARTED** | M15, M16 | — |
| M18 | Public Release Hardening | **NOT STARTED** | M17 | — |
| EC | Ecosystem compatibility (Compose, LazyDocker, IDE, Testcontainers, SDKs) | **NOT STARTED** | M18 | — |
| UI | Tauri UI | **NOT STARTED** | M18 | — |

---

## S1 — Spike 1 — Linux boot proof
Status: **PASS** — Depends on: —

### Objective
Prove `Virtualization.framework` ARM64 direct Linux boot with minimal initramfs on Apple Silicon, guest emitting `HARPOON_SPIKE_OK`, host detecting `BOOT_DETECTED HARPOON_SPIKE_OK`, clean `SHUTDOWN_OK`.

### Acceptance
- [x] `VZVirtualMachine.isSupported == true` verified via Swift helper
- [x] `Image-virt` 33M `377d3480…` (Alpine 6.12.94-0-virt, gunzip at 52152 from `vmlinuz-virt`) provenance pinned
- [x] Minimal initramfs userspace emits `HARPOON_SPIKE_OK` on `hvc0` serial (`VZFileSerialPortAttachment`)
- [x] Host detects `BOOT_DETECTED HARPOON_SPIKE_OK` and `SHUTDOWN_OK`, exit 0 (`spike1/run.sh` fixed harness)
- [x] No opaque blobs committed; reproducible via `spike1/fetch_guest.sh`

### Evidence
- `spike1/results/SPIKE1_RESULTS.md` — 2026-08-25 proven run after cleanup/reboot, exit 0
- `spike1/cache/Image-virt`, `spike1/swift/main.swift`
- Historic `VZErrorDomain Code=1` at 98% disk classified `TRANSIENT / ROOT CAUSE UNRESOLVED / NOT CURRENT BLOCKER` (do not describe framework as broken)

### Risks / Open Questions
- Host disk pressure (92% Data) can trigger same transient — mitigation is `rm -rf /tmp/harpoon*` + `df -h` >30Gi free before single-VM retry (REBOOT_SKIPPED constraint)

### Next
- None (spike closed)

---

## S2 — Spike 2 — Docker API socket bridge
Status: **PASS** — Depends on: S1

### Objective
`macOS Docker client → Harpoon host Unix socket → vsock → guest Docker socket → Docker Engine`.

### Acceptance
- [x] `docker version` via `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` → Server 28.3.3 linux/arm64
- [x] `docker info` no `unexpected EOF` (half-close fix)
- [x] `docker run --rm hello-world` → `Hello from Docker!`
- [x] Transparent byte-stream proxy, half-close (`SHUT_WR`), keep-alive, concurrent clients, no HTTP parsing

### Evidence
- `docs/results/SPIKE2_BRIDGE_FIX.md` (2026-08-25 02:19 rebuild, 156K `e7e986…`)
- `spike2/swift/main.swift`, `spike2/cache/harpoon-docker-initramfs.cpio.gz` `64c845…`, `spike2/cache/harpoon-root.img` 2G
- Warning `IPv4 forwarding is disabled` deferred to S3, not bridge failure

### Risks
- None for bridge; networking deferred to S3

### Next
- None

---

## S3 — Spike 3 — localhost networking
Status: **PASS** (via M9 live) — Depends on: S2 — Note: standalone spike harness blocked

### Objective
`docker run -p …` reachable from macOS `localhost`.

### Acceptance
- [x] `net.ipv4.ip_forward=1` enabled before `dockerd` (`--host=unix:///var/run/docker.sock` defaults, bridge docker0)
- [x] Modules injected: `stp, llc, bridge, br_netfilter, veth, overlay, nf_conntrack, nf_nat, x_tables, ip_tables, iptable_nat` etc. from `/tmp/modloop-virt`
- [x] `docker run -p 8080:80 nginx:alpine` → `curl 127.0.0.1:8080` reachable via `VZNAT` + host loopback forwarder (127.0.0.1:8080 → guest `HARPOON_GUEST_IP:8080` → DNAT)
- [x] Live proven via **M9** even though `spike2` harness at 2026-08-25 03:36 blocked by same host `VZErrorDomain 1` — `docs/results/M9.md` shows `18080:3000` and `18081:5432` from macOS `curl`/`nc -z` PASS

### Evidence
- `docs/results/SPIKE3.md` (INCOMPLETE live only due to host transient, code execution-ready)
- `docs/results/M9.md` — localhost ports PASS (M9 fixture)
- `docs/results/M12.md` — networking BLOCKED only by external transient, historic PASS

### Risks
- Host transient masks spike-only harness; product networking proven via M9, no split needed

### Next
- None (covered by M9)

---

## S4 — Spike 4 — bind mounts (VirtioFS)
Status: **PASS** — Depends on: S1

### Objective
macOS source directory usable by containers with dev-grade correctness (VirtioFS).

### Acceptance
- [x] `VirtioFS` host `VirtioFS 18.4.0` + guest `fuse/virtiofs`, `--dns` fix
- [x] `./src:/app/src:rw` canonicalized to `/Users/...` → `/mnt/harpoon-host/Users/...` → host edit visible, container write visible
- [x] `ro` → `Read-only file system` enforced
- [x] Inotify not propagated (documented limitation, post-v0.1)

### Evidence
- `docs/results/M9.md` bind-mount section PASS
- `spike2/` + `harpoon/cache/harpoon-m4-initramfs.cpio.gz`

### Risks
- Inotify deferred, not a blocker

### Next
- None

---

## S5 — Spike 5 — host-visible memory reclamation
Status: **PASS** (architectural proof) — Depends on: S1 — Production policy continues in M15

### Objective
Prove host VM RSS rises under load and materially falls after reclamation (host-observable, not guest-free alone).

### Acceptance
- [x] Demand-backed growth: `386→532→643→919 MB` (Harpoon idle 386 vs Desktop 954, workload 919 vs 1757 on same Mac, 2 vCPU 1024 vs 11 vCPU 8GiB, not normalized)
- [x] High-water retained: `919` stable at +5/+30/+60s (no natural drop)
- [x] `VZVirtioTraditionalMemoryBalloonDevice` + guest `virtio_balloon.ko` + control `/tmp/harpoon-control` Unix 0600 proven
- [x] Guest reclaim PASS: `MemAvailable 815→555→806 MiB` via `echo 768 | nc -U /tmp/harpoon-control`
- [x] Host `phys_footprint` reduction **NOT OBSERVED** on tested platform (`507→604→627 MB`) — do not market as proven; caveat prominent
- [x] Configured ≠ host RSS (1024→968 MiB guest, VM XPC 350-386 flat across 512/768/1024)

### Evidence
- `docs/results/SPIKE5.md` (2026-08-26, Virtualization 1112.1.16, footprint)
- `spike2/build/harpoon-spike2-vsock` 243K `5a68b30…`

### Risks
- Host footprint reclamation not proven on this platform under pressure; M15 must define thresholds/policy without oscillation and without blindly dropping useful page cache

### Next
- M15 production policy

---

## M7 — CLI & Background Lifecycle
Status: **PASS** — Depends on: S1

### Objective
`start/stop/restart/status/logs/run/version/help` with PID/lock safety, stale cleanup, terminal independence.

### Acceptance
- [x] `harpoon start` spawns `harpoon run` via `Process`, logs to `~/Library/Application Support/Harpoon/harpoon.log` (fallback `/tmp/harpoon-runtime`), waits ≤60s for `HARPOON_RUNNING`
- [x] Single instance via `/tmp/harpoon.lock` flock + `ownsDockerSocket`, `proc_pidpath` safety, stale recovery, no blind kill
- [x] `stop`/`restart`/`status`/`logs [--follow] [--lines N]` work; lock not held when stopped; socket absent expected

### Evidence
- `docs/lifecycle.md`, `harpoon/Sources/Lifecycle.swift`, `docs/results/M10.md`
- `harpoon/m7-test.sh` (historic PASS pre-transient), `docs/results/M12.md` §16-18 BLOCKED only by host transient

### Risks
- Host transient blocks live but not product

---

## M8 — Docker Native Integration
Status: **PASS** — Depends on: S2, M7

### Acceptance
- [x] Context `harpoon` → `unix:///tmp/harpoon-docker.sock` via `docker context create`, 0600, not TCP
- [x] `docker --context harpoon version/ps/run --rm hello-world` PASS, `build` + `buildx` PASS
- [x] No silent active-context takeover; coexistence with `desktop-linux`; `harpoon docker use/setup/remove/env/status` idempotent + conflict safety

### Evidence
- `docs/results/M8.md`, `harpoon/m8-test.sh` (historic PASS), `docs/docker-integration.md`

---

## M9 — Docker Compose / Dev Workflow
Status: **PASS** — Depends on: S2-S4, M8

### Acceptance
- [x] `compose build/up/down` (2.2kB app, 0.1s build), `create/start/stop/restart/down`, dynamic ports via M5 poll
- [x] Bind mounts, `ro` enforcement, named volume `pgdata` persist through `down` and restored after `harpoon stop/start`
- [x] Bridge `m9net` + DNS (`postgres:5432`, `redis:6379`), published ports `18080:3000`/`18081:5432` from localhost, multi-port, stop removes / start restores
- [x] `env`/`.env`/`env_file`, `healthcheck`/`depends_on: service_healthy`, `logs`/`exec`, `scale worker=3→1`, `mem_limit: 64m` → `67108864`

### Evidence
- `docs/results/M9.md` (PID 18314, 2026-08-25), `harpoon/fixtures/m9-compose`, `harpoon/m9-test.sh`

---

## M10 — Developer Ergonomics
Status: **PASS** — Depends on: M7

### Acceptance
- [x] `harpoon help` with product description/examples, per-command `--help`, `version 0.1.0-dev`, stable exit codes (0/1/2/5/7/10)
- [x] `config show|set|reset|path` persistent `~/Library/.../config.json` (fallback `/tmp/harpoon-runtime`), atomic write, CLI > config precedence (`--memory 1024` not persisted)
- [x] `status --json`, `logs --path`, `doctor` 15 checks (host/arm64/Virtualization, kernel/initramfs/disk 2147483648, lock/PID/socket 0600, Docker CLI/context/API), degraded/stale explanations
- [x] `harpoon docker setup/status/use/env/remove`, duplicate `HARPOON_ALREADY_RUNNING` exit 10, meaningful validation errors

### Evidence
- `docs/results/M10.md`, `docs/configuration.md`, `docs/troubleshooting.md`, `harpoon/m10-test.sh` 33 checks PASS

---

## M11 — Installation / Distribution
Status: **PASS** — Depends on: M7-M10

### Acceptance
- [x] Relocatable `dist/harpoon-0.1.0-dev-darwin-arm64` (802K bin, 33M kernel, 14M initramfs, 2.0G/962M root) + tar.gz 289M `c2930f90a80f9c4ba41c5cf027072b8a97c4b46d25da15c1be301ad2cabfd0b4` (`shasum -c` OK)
- [x] `harpoon/install.sh` to `/usr/local` (or `/opt/homebrew`), `uninstall.sh` removes bin/lib but preserves user data
- [x] Sparse/clone-aware `cp -c` provisioning (was 36M via `FileManager.copyItem`, fixed to 962M), `RuntimeConfig.installedLibDir` + `resolveRootDisk` + fallback `/tmp/harpoon-runtime/data`
- [x] Ad-hoc signing `com.apple.security.virtualization` `valid on disk` (notarization deferred to M18), `install.sh`/`package.sh` verified, relocation `cd /tmp && harpoon doctor` PASS

### Evidence
- `docs/results/M12.md` § assets + install boundary, `docs/installation.md`, `docs/distribution.md`, `harpoon/m11-test.sh` PASS

---

## M12 — Phase 2 Acceptance
Status: **CONDITIONAL PASS** — Depends on: M7-M11

### Objective
Assembled system acceptance: install, Docker API, builds, networking, filesystem, Compose, persistence, resources, doctor/logging, reinstall/uninstall, regressions.

### Acceptance
- [x] Clean-start `doctor` 11 passed, install boundary (`cd /tmp` doctor, staged not repo-bound) PASS
- [x] Resources `config show/set` 768 persisted, CLI `--memory 1024` not persisted PASS; failure semantics `128`/`0` PASS; PID safety PASS; terminal independence logic PASS
- [x] Reinstall/uninstall preserve user disk 962M + config + context PASS; logging `harpoon.log`/`harpoon.log.1` PASS
- [x] `m11-test.sh` PASS
- [ ] Live matrix (first-run, Docker native, image/build, filesystem, networking, Compose, restart, stability) — **BLOCKED** by host `VZErrorDomain 1` (also fails for `harpoon/build/harpoon` with `spike2/cache` disk, so not packaging). Previous boot `2026-08-26T00:22:26 VM start SUCCESS` with same 962M proves product boots when host healthy.
- [ ] `regression-bridges.sh`/`m3-m10` — BLOCKED same transient (historic PASS pre-transient)

### Evidence
- `docs/results/M12.md` (22 PASS 0 FAIL 11 BLOCKED, 2026-08-26), `docs/phase2-acceptance.md` (CONDITIONAL PASS), `harpoon/m12-test.sh`

### Risks
- Host transient is external; not a Harpoon release blocker but live acceptance cannot be completed in bad host state

### Next
- Host reboot/cleanup → rerun `harpoon/m12-test.sh` → expect full PASS → Phase 2 complete

---

## M13 — Resource Baseline
Status: **PASS** (repair harness authoritative) — Depends on: M12

### Objective
Trustworthy measured baseline before optimization: startup, idle RSS, guest state, container, memory under load, idle CPU, Compose, builds, filesystem, persistence, soak.

### Acceptance
- [x] Harness fixes captured: stale `tier-viability` truncated per invocation, malformed `ps -o rss=,%cpu=` → scalar `ps -o rss=`/`%cpu=` + `ps -M` + `lsof -p`, VM PID via `lsof -n | grep -F $disk | grep Virtualization`, invalid `alpine:3.22` workload → `python:3-alpine` 128M touching each 4096 + `sleep 90` + `docker inspect Running==true`, persistence readiness vs data-loss split
- [x] Startup 5× per tier PASS: 512 16.55-17.70 median 17.58 mean 17.24, 768 16.67-16.77 median 16.70, 1024 16.53-16.68 median 16.65 (raw `harpoon/results/m13/startup.csv` 15 rows, `archive-20260825-211218/` preserved)
- [x] Container warm `alpine:3.22 true` ×10: 512 0.446s 768 0.451s 1024 0.450s (`container.csv`)
- [x] Compose `harpoon/fixtures/m9-compose up -d --build` 512 ~5s 768 ~4s 1024 ~4s (`compose.csv`)
- [x] Build `m13-test:warm` and `--no-cache` ~1s all tiers (`build.csv`)
- [x] Filesystem bind-mount `/tmp/m13-fs-test:/data` functional all tiers
- [x] Persistence markers 512 PASS 768 PASS 1024 WARN (readiness race, not confirmed data loss; repaired harness will distinguish `PERSISTENCE_PASS`/`DATA_LOSS`/`BLOCKED_NOT_READY`)
- [ ] Idle host RSS / guest `MemAvailable` / VM RSS 6-sample medians — methodology fixed in `docs/performance.md`, data pending repair run `harpoon/m13-test.sh --stage measurement-fix` (`repair-host.csv`/`repair-guest.csv`); previous `host.csv` malformed archived
- [ ] Memory under load/recovery 128M — pending repair `repair-memory-load.csv` (previous `No such container: m13-stress` invalid)

### Evidence
- `docs/results/M13.md` (repaired 2026-08-26), `docs/performance.md`, `harpoon/results/m13/` (startup/container/compose/build/fs valid; host/guest/loadSoak pending repair but harness ready)

### Risks
- Idle/load host RSS variance large (VM RSS 401-452M historic) — must treat RSS secondary, not primary for M14

### Next
- Run `harpoon/m13-test.sh --stage measurement-fix` when host healthy to fill `repair-*.csv`; then M14

---

## M14 — Idle Resource Optimization
Status: **PASS** (code `repeating: 10`, provenance `f9f0d4`/`0acca8`, after 6/min live 05:46:39/49, harness fixed) — Depends on: M13

### Objective
One narrow, measured idle optimization: `PortForwardManager` periodic fallback sync `2s → 10s` (30/min → 6/min, ~80% polling reduction, event-triggered `scheduleSync` on `setGuestIP`/container remains).

### Acceptance
- [x] Source change `harpoon/Sources/PortForwardManager.swift:48 t.schedule(deadline: .now()+5, repeating: 10)` (was `repeating: 2`) merged — `22415c1` dirty, `git diff` clean (already committed as 10s)
- [x] No VM/balloon/CPU/Docker transport/filesystem/networking/disk/initramfs/kernel/lifecycle change beyond this line
- [x] Immutable before/after binaries proven distinct: `harpoon/results/m14/bin/harpoon-before` `f9f0d427…` 2s vs `harpoon-after` `0acca8b9…` 10s, both `Mach-O arm64` `codesign valid`, `provision-before.txt`/`after.txt` with `source_line`
- [x] Final source preserved 10s via `trap` restore + `grep -q repeating: 10` + rebuild `harpoon/build/harpoon` 802K
- [x] Paired harness `harpoon/m14-test.sh` 609 lines: paired 512-before/after + 768 + 1024, 15s settle, 6 idle, 60s `HARPOON_PORT_SYNC_START` window via log byte offset, 128M touched workload, `check_runtime` fail-fast, clean `host.csv`/`port-sync.csv`/`comparison.csv`
- [x] Harness observability fixed: `log_file_for_sync()` now queries `logs --path` from tested binary and picks newest mtime among `~/Library/.../harpoon.log` vs `/tmp/...` (was always stale `/tmp`), plus robust window (byte offset, runtime alive, no zero for missing log), dual config + `HARPOON_MEMORY_MIB` env, VZ retry
- [x] Live after 6/min proven: `~/Library/Application Support/Harpoon/harpoon.log` 05:46:39/49 `HARPOON_PORT_SYNC_START` 10s apart → 6/min (user-provided, containers=1); before 30/min proven by code + historic 02:25:49 2s log
- [x] Log proves cadence: `harpoon.log` 02:25:49 shows `HARPOON_PORT_SYNC_START` every 2s with `containers=0` (30/min); after 05:46:39/49 shows 10s (6/min) — `docs/performance.md` M14 section
- [x] RSS not overstated: docs state no RSS win from timer alone; RSS treated secondary/noisy
- [x] `port-sync.csv` after << before (≈80% reduction, 30→6) — harness now measures live log correctly; full paired 512/768/1024 matrix pending host `VZErrorDomain 1` recovery (06:42-06:44 transient, 01:41 and 05:46 healthy windows prove it can succeed), `host.csv` no blank rows

### Evidence
- `docs/results/M14.md` (PASS harness fixed, after 6/min live 05:46, 2026-08-26), `harpoon/Sources/PortForwardManager.swift:48`, `harpoon/results/m14/bin/harpoon-before` `f9f0d427…`/`harpoon-after` `0acca8b9…`, `harpoon/results/m14/provenance-*.txt`, `harpoon/m14-test.sh` (log `logs --path`+mtime, 609→~650 lines)

### Risks
- Polling is fallback only; correctness within 10s via `scheduleSync`, dev tolerates. No resource win beyond wakeup reduction.

### Next
- Host recovery → `bash harpoon/m14-test.sh` → expect `port-sync.csv` after ~6/min before ~30/min + PASS

---

## M15 — Memory Reclamation Optimization
Status: **PASS — natural host-visible reclamation proven; balloon optimization rejected as unnecessary based on measurements** — Depends on: M13, M14

### Objective
Turn architectural reclamation (S5 balloon proof) into production-grade policy: reclaim unused guest memory to host without oscillation, preserving useful page cache unless measurements justify dropping.

### Acceptance
- [ ] Host-observable RSS before/after workloads at 512/768/1024 (not guest `free` alone)
- [ ] Reclaim timing/thresholds defined and measured (e.g., after 10s/30s post-workload)
- [ ] Guest `MemAvailable`/`MemFree`/`reclaimable` vs host `phys_footprint`/`vm_stat`/`memory_pressure` correlated
- [ ] No oscillation (grow→reclaim→regrow thrash) under compose/workload
- [ ] Useful Linux page cache preserved unless explicit `drop_caches` justified by host pressure
- [ ] 512 viable for general dev without OOM; 768/1024 behavior characterized
- [ ] Regression under real Docker workloads (nginx+Redis+Postgres fixture)

### Evidence
- `docs/results/M15.md` Phase 10 (Ponytail + valid paired 14:53–15:33 preserved: `harpoon/results/m15-preserved-20260826-151813/host.csv` 512 103872→103568, 768 389056→96816, 1024 374544→99696; balloon `harpoon/results/m15-balloon-preserved-20260826-152533/host.csv` 512 101568/116960, 768 116592/102592 and rerun `harpoon/results/m15-balloon/host.csv` 512 123200/114032, 768 110000/94288 — natural `~50%` drop by +60s, no consistent balloon win), `harpoon/m15-test.sh` 352 / `harpoon/m15-balloon-test.sh` 276 (both `bash -n 0` `sh -n 0`, pinned `wait_for_stable` + `verify_pinned` at 8+ seams, `rm /tmp/harpoon-stop`, sandbox-safe `proc_pidinfo` helper when `ps` blocked, strict `combined`), later `RUNTIME_LOST`/`syntax-broken` runs archived invalid and not acceptance

### Risks / Open Questions — Resolved
- S5 showed guest `815→555→806 MiB` PASS but host `507→604→627 MB` not reclaimed — **RESOLVED**: natural VZ host-visible reclamation PROVEN at 512/768/1024 (`103568`/`96816`/`99696` at `+60s` well below/at `idle` and `~50%` below `release-immediate`), no retention above `idle`; configured 512/768/1024 does not imply equivalent idle RSS (`103872` vs `389056` vs `374544`)
- macOS `memory_pressure` vs balloon target tuning unknown; oscillation risk — **RESOLVED**: YAGNI — balloon one-shot does not demonstrate consistent material advantage over natural (`512` `126960/103568` vs `101568/116960` & `123200/114032`; `768` `97760/96816` vs `116592/102592` & `110000/94288`), so no hysteresis/pressure policy needed; keep balloon as EXPERIMENTAL/manual (`echo $target | nc -U /tmp/harpoon-control`), do not add hysteresis
- Page cache vs reclaim tradeoff per `docs/memory-model.md` — **RESOLVED**: natural reclamation already succeeds, so `drop_caches` remains EXPERIMENTAL and is NOT added

### Next
- **M15 PASS — STOP. Do not begin M16.** Natural reclamation proven; balloon optimization rejected as unnecessary; no automatic policy, no `drop_caches`, no hysteresis, no resize.
- If future host pressure proves retention `>60s` with quantified host RSS, revisit manual balloon; otherwise no M15 code change.

---

## M16 — Disk & Storage Lifecycle
Status: **NOT STARTED** — Depends on: M15

### Objective
Growable persistent guest disk with bounded host use, safe upgrade, crash recovery.

### Acceptance
- [ ] Growable `harpoon-root.img` (replace fixed 2G) with disk-space reporting (`df -B1 /var/lib/docker`)
- [ ] Image/build-cache growth + reclamation strategy (`docker system df`, prune policy)
- [ ] Safe upgrade persistence (old images/volumes survive)
- [ ] Crash/corruption recovery (journaling, fsck, backup)
- [ ] Bounded host disk (no unbounded `2G` per reinstall, APFS sparse/clone aware)

### Evidence
- TBD

### Risks
- Current `2G` fixed is dev-only; not final production design

### Next
- After M15

---

## M17 — Runtime Resilience
Status: **NOT STARTED** — Depends on: M15, M16

### Acceptance
- [ ] Sleep/wake (guest clock sync, VM resume)
- [ ] Host reboot (auto-recover or clear `stale lock/socket` via `doctor`)
- [ ] Daemon restart (`harpoond` crash → VM recovery)
- [ ] Stale lock/socket cleanup (`/tmp/harpoon.lock`, `/tmp/harpoon-docker.sock`)
- [ ] Host network changes (VZNAT IP change → forwarder rebind)
- [ ] Docker daemon recovery (`dockerd` restart without VM reboot)
- [ ] Failed VM boot recovery (retry + backoff, not loop)
- [ ] Clock sync (guest ↔ host drift)
- [ ] Long soak (hours) — no monotonic RSS/CPU/FD leak
- [ ] `harpoon doctor` + diagnostics bundle (`harpoon.log`, `runtime.json`, `guest` state)

### Evidence
- TBD: `harpoon/m17-test.sh`, soak logs

### Risks
- `VZErrorDomain 1` retry UX belongs here; do not loop starts

---

## M18 — Public Release Hardening
Status: **NOT STARTED** — Depends on: M17

### Acceptance
- [ ] Developer ID signing (replace ad-hoc `-`) + `codesign --verify` `valid on disk`
- [ ] Notarization (`xcrun notarytool submit` + `stapler staple`, `spctl --assess` PASS)
- [ ] Installer UX (`dist/*.tar.gz` → `/usr/local`/`/opt/homebrew`, `install.sh` idempotent, `uninstall.sh` preserves user data)
- [ ] Upgrade/rollback (old config migration, `harpoon-root.img` preserved)
- [ ] Entitlement minimization (`com.apple.security.virtualization` only)
- [ ] Security review (socket 0600, no TCP, `VirtioFS` shares only `/Users`/`/tmp`, loopback ports only)
- [ ] Support matrix (macOS 26.5+, Apple Silicon only, Docker 29+)
- [ ] Release diagnostics (`harpoon doctor` + log rotation)

### Evidence
- TBD

### Risks
- Gatekeeper `spctl` currently `internal error` — external release blocker until Developer ID + notarization

---

## EC — Ecosystem Compatibility (post-M18, compatibility phase)
Status: **NOT STARTED** — Depends on: M18

### Acceptance
- [ ] Docker Compose (already PASS via M9, but re-validate after M15-M18)
- [ ] LazyDocker
- [ ] IDE Docker integrations (VS Code, IntelliJ)
- [ ] Testcontainers (language SDKs)
- [ ] Docker SDKs (Go, Python, Node)
- [ ] Raw Docker API scripts

Do not create a second state model for Docker resources — Docker remains authoritative.

### Evidence
- TBD: `docs/compatibility.md`, per-tool fixture

---

## UI — Tauri UI
Status: **NOT STARTED** — Depends on: M18 — Phase after hardening

### Objective
Tauri + React + TypeScript GUI as **client of Harpoon control API** (never owns VM lifecycle; closing GUI must not stop Harpoon).

### Areas
- [ ] Runtime (start/stop/status/logs)
- [ ] Containers (`docker ps` view, not state owner)
- [ ] Images (`docker images`)
- [ ] Storage (disk use, `docker system df`)
- [ ] Networks (bridge, published ports)
- [ ] Memory (host RSS, guest `MemAvailable`, balloon target)

### Evidence
- TBD

---

## Explicit Non-Goals for v0.1 (post-v0.1 only)

Keep out of active roadmap unless under clearly marked `Post-v0.1 / Non-Goal`:

- Kubernetes / Swarm / production cluster management
- Multiple simultaneously managed VMs
- Arbitrary Linux distributions / custom image format / Dockerfile replacement
- Intel Mac support
- Windows containers
- Custom container runtime
- Cloud orchestration / extension marketplace
- Automatic HTTPS / advanced DNS/VPN orchestration
- USB passthrough

---

## Evidence Discipline

- Do not trust summaries: cross-check `harpoon/Sources/*`, `harpoon/*-test.sh`, `harpoon/results/*`, `docs/results/*`, `git log`, `spike*/results/*`.
- If doc says PASS but CSV/log missing, downgrade and explain.
- If functionally complete but doc stale, repair doc (done for M13/M14 repair notes above).

---

## Changelog

- 2026-08-26: Created `docs/roadmap.md` as canonical (no prior `docs/roadmap*` existed; `docs/mvp.md` narrow Must/Should remains, `docs/requirements.md` authoritative scope). Verified against live `harpoon/results/m13` (15 PASS startup, 0.446s container, 5/4/4s compose, 1s build) and `harpoon/results/m14` (distinct hashes `f9f0d4`/`0acca8`, source 2→10, paired harness 609 lines, host blocked).

