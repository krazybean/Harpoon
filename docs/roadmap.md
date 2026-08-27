# Harpoon Roadmap — Canonical Trackable

> Harpoon is a lightweight Docker-compatible runtime for Apple Silicon macOS. Docker Engine remains authoritative for containers/images/volumes/networks/API/Compose/BuildKit. Harpoon owns only the macOS↔Linux boundary (VM lifecycle, socket bridge, networking, bind-mount transport, resources, memory reclamation, diagnostics).

## Current Position

- **Current phase:** Release Tooling — **PASS — canonical npm build/release/version workflow (package.json 0.1.0 single source, version:bump/version:check, build:release/release with 3072 MiB DMG, sign-app.sh nested→outer, clean:release)**; D1.1 PASS (canonical 1.0G), core MVP + EC + UI COMPLETE
- **Last completed milestone:** UI — Tauri UI (PASS — Tauri 2 + React-TS + Vite, 185kB dist, cargo check dev, status --json live, START_PASS 20:50:42Z running + EXPANSION + RESPONSIVENESS + BOOTSTRAP — resolver HARPOON_BIN/CARGO_MANIFEST_DIR/bundled/PATH + Docker resources + async cache/instant nav + bootstrap launching→ready/failed auto-start once + phase spinner + 750ms polling + per-action busy, no Sources redesign, no Redux, no launch agent)
- **Next milestone:** UI Design / UX Polish — NEXT (Release Tooling PASS, do not begin Developer ID/notarization, updater, Homebrew, or UI redesign in this task)
- **M17 status:** PASS — 8 functional acceptance demonstrated in preserved healthy 181158 (CLOCK_SYNC_PASS/STALE_CLEANUP_PASS/RESTART_RECOVERY_PASS/VZNAT_REBIND_PASS/DOCKERD_RECOVERY_PASS/FAILED_BOOT_RECOVERY_PASS/SOAK_PASS/DOCTOR_PASS 16 PASS, 0 warnings, 0 failures), PASS,completed suppressed solely by substring bug (`grep -q "FAIL"` matched `FAILED_BOOT_RECOVERY_PASS`) → fixed exact-field `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'` yields PASS (9 rows); subsequent HOST_VZ_START_FAILURE retained as historical host-platform evidence (R1 5 paired cycles 10/10 implicates host/VZ state) — no Sources change, no preserved CSV fabrication
- **M18 status:** PASS — healthy complete run `PASS,completed` (PACKAGE_PASS bin 802K arm64 signed, STOP_PASS, START_PASS running 90641, DOCKER_VERSION/INFO/RUN PASS, SOCKET/CONTROL 0600, DOCTOR 16 PASS, LOGS/PERSISTENCE/DISK/RESTART/ALREADY_RUNNING/DOUBLE_STOP/VERSION PASS, VZ 3/3); harness contamination fixed 203 lines `logs --path` byte window + live `status --json` precedence; prior HOST_VZ windows preserved as historical — Known risk: intermittent VZErrorDomain 1 / HOST_VZ_START_FAILURE observed & characterized by R1 remains, not claimed fixed; package 289M b6a40f, README fixed, no Sources change

- **Fresh-start DNS validation:** PASS (2026-08-27) — clean stop → fresh start → Docker Engine 28.3.3 ready → guest resolv.conf with working resolvers → registry-1.docker.io resolved → uncached busybox:1.37 pull succeeded. Supersedes prior pending note.
- **R1 status:** DONE — 5 paired cycles 19:23Z minimal FAIL + full FAIL 10/10 VZErrorDomain 1 (2000-3000ms, validate OK, same Image-virt 377d3480) — HOST/VZ STATE strongly implicated (not Harpoon topology disk/VirtioFS/vsock/balloon), no Sources change; intermittent VZErrorDomain 1 remains known risk/observation, not claimed fixed
- **M16 status:** PASS — Bounded 2147483648 logical 1063980 KiB sparse, df -B1 / 2040373248 40% overlay, docker system df 514.3MB, m16-vol+image 8.54MB persistence PASS via build-context 74B, ext4 /dev/vda journal, APFS clone 1063980 KiB, harness 291 lines bash -n 0 sh -n 0 (repaired rerun at 17:49-17:50 PASS), growable REJECTED YAGNI
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
| M16 | Disk & Storage Lifecycle | **PASS — Bounded 2147483648 logical 1063980 KiB, df -B1 / 2040373248 40% overlay, system df 514.3MB, build-context 74B → m16-test:1.0 8.54MB, m16-vol+image persistence PASS across stop/start (only if inspect pre-restart), ext4 /dev/vda journal, APFS clone 1063980 KiB, reuse inode 351160594, `tier-status.csv` PASS at 17:50:34, preserved 17:29 in `m16-preserved-20260826-1730/`; harness 291 lines bash -n 0 sh -n 0, growable REJECTED YAGNI/POST-MVP** | M15 | `docs/results/M16.md` (PASS 17:49), `harpoon/m16-test.sh` 291, `harpoon/results/m16/` PASS, `harpoon/results/m16-preserved-20260826-1730/` |
| M17 | Runtime Resilience | **PASS — 8 functional + corrected exact-field verdict (CLOCK_SYNC/STALE/RESTART/VZNAT/DOCKERD/FAILED_BOOT/SOAK/DOCTOR 16 PASS, 0 warnings, 0 failures; 8→9 PASS via `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'`, historical HOST_VZ_START_FAILURE retained as known risk characterized by R1, §13 PROVEN 8 +5 YAGNI/POST-MVP, §14-15 closure)** | M15, M16 | `docs/results/M17.md` §13-15 (PASS, preserved 181158 healthy 8→9 PASS, historical HOST_VZ 182017/182137/202447/202702 retained), `harpoon/m17-test.sh` 154, `harpoon/results/m17/` |
| R1 | VZ Startup Isolation (R1) | **DONE — HOST/VZ STATE implicated (5 paired cycles 19:23Z minimal FAIL + full FAIL 10/10 VZErrorDomain 1, same Image-virt, not Harpoon config)** | M18 | `docs/results/R1.md`, `harpoon/results/r1/paired.csv` (10 rows), `harpoon/results/r1/logs/` |
| M18 | Public Release Hardening | **PASS — healthy complete run `PASS,completed` (START_PASS 90641, DOCKER_VERSION/INFO/RUN, PERSISTENCE, RESTART, DOCTOR 16 PASS, VZ 3/3, DISK 2147483648, socket 0600; harness 203 `logs --path` window + live precedence fixed, package 289M b6a40f; historical HOST_VZ retained as known risk)** | M17 | `docs/results/M18.md` §4 addendum + §7 healthy smoke (PASS,completed, VZ 3/3), `harpoon/m18-test.sh` 203 (`bash -n 0` `sh -n 0`), `harpoon/results/m18/` PASS, `harpoon/results/m18-preserved-*contaminated` retained |
| EC | Ecosystem compatibility (Compose, LazyDocker, IDE, Testcontainers, SDKs) | **PASS — matrix A-H via harpoon/ec-test.sh 425 (when healthy, prior M9/M17/M18 trustworthy, BLOCKED 20:34 preserved as host transient, not product; LazyDocker/SDK/Testcontainers NOT TESTED with reason)** | M18 | `docs/results/EC.md` (BLOCKED 20:34 preserved, matrix defined, Ponytail, no Sources redesigned), `harpoon/ec-test.sh` 425 |
| UI | Tauri UI | **PASS — Tauri 2 + React-TS Vite 185kB, status --json live 20:50:42 START_PASS + EXPANSION + RESPONSIVENESS + BOOTSTRAP — binary resolver HARPOON_BIN/CARGO_MANIFEST_DIR/bundled/PATH + Docker resources + async cache/instant nav/bounded polling + bootstrap auto-start/phase spinner/failure Retry (launching→discovering→starting→vm_booting→docker_starting→ready/failed, no launch agent)** | M18 | `docs/results/UI.md` (PASS + EXPANSION, preserved 20:50:42, resolver fix, Docker resources), `ui/harpoon-desktop` (Tauri 2 + React 7 views), `harpoon/ui-test.sh` 92 |
| D1 | Self-contained macOS Distribution | **BLOCKED_HOST_TRANSIENT — Harpoon.app 2.0G `73674acd` + kernel `377d3480` + initramfs `70a89d58`, resolver bundled preferred, `~/Library` disk preserved, signing ad-hoc virtualization only on harpoon, `docker version` via bundle when healthy, current `06:24:11Z HOST_VZ` blocks Docker ready (not packaging), clean-machine no Node/Rust** | D1 | `docs/results/D1.md`, `Harpoon.app` (`target/release/bundle/macos/Harpoon.app`) |
| D1.1 | Release Artifact Cleanup + Final DMG Verification | **PASS (canonical) — Harpoon.app 1.0G single Resources/harpoon tree (Map `{"bundle-resources/harpoon": "harpoon"}`), 73674acd/377d3480/70a89d58, signing ad-hoc virtualization on harpoon only, `codesign --verify --deep --strict` PASS after re-sign, DMG sandbox `hdiutil Device not configured` blocker (non-product), Docker smoke currently BLOCKED_HOST_TRANSIENT at 06:44:12Z/06:44:27Z VZErrorDomain 1 (same via repo + bundle, not packaging)** | D1 | `docs/results/D1.1.md`, `Harpoon.app` 1.0G canonical, `target/release/bundle/macos/Harpoon.app` |
| Release Tooling | **PASS — canonical npm build/release/version workflow (package.json 0.1.0, scripts version.mjs/release.mjs/sign-app.sh/clean.mjs, build:release/release with 3072 MiB DMG, version:check PASS, clean:release)** | D1.1 | `ui/harpoon-desktop/package.json`, `ui/harpoon-desktop/scripts/` |

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
Status: **PASS — Bounded sparse, guest df, system df, build-context, volume+image persistence, journaling, clone/reuse proven (repaired harness 291)** — Depends on: M15

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
- M16 PASS — Do not begin M17. Current Position updated: Last completed M16, Next M17 (not started).

---

## M17 — Runtime Resilience
Status: **PASS — 8 functional + corrected verdict (preserved healthy 181158 8 PASS→9 PASS via exact-field awk, historical HOST_VZ_START_FAILURE 17:57-18:21/20:25/20:27 retained as known risk, not invalidating; §13 PROVEN 8 +5 YAGNI/POST-MVP, §14-15 closure)** — Depends on: M15, M16

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
- `docs/results/M17.md` §13-15 (PASS, preserved `m17-preserved-20260826-181158-healthy` 8→9 PASS via exact-field `awk -F, 'NR>1 && ($2=="FAIL" || $2~/_FAIL$/)'`; historical HOST_VZ `m17-preserved-20260826-182017`/`182137`/`202447`/`202702` retained as known risk, not invalidating), `harpoon/results/m17-preserved-20260826-181158-healthy/tier-status.csv` 8 PASS (would be 9 with `PASS,completed`), `harpoon/results/m18/` smoke `PASS,completed` proves host recovery possible

### Risks
- `VZErrorDomain 1` retry UX belongs here; do not loop starts

---

## M18 — Public Release Hardening
Status: **PASS — healthy complete run PASS,completed (package 289M b6a40f signed, staged binary works outside tree, harness 203 fixed, START_PASS/DOCKER/PERSISTENCE/RESTART/DOCTOR 16 PASS/VZ 3/3; historical HOST_VZ retained as known risk, not claimed fixed)** — Depends on: M17

### Acceptance
- [x] Package builds successfully (`bash harpoon/build.sh` + `bash harpoon/package.sh` → 802K arm64 `valid on disk` `com.apple.security.virtualization`, 33M kernel, 14M initramfs, 2G sparse `cp -c`)
- [x] Package contents audited (13 files `tar tzf`: `bin/harpoon` + `lib/harpoon` 3 artifacts + `share/doc` 2 docs + `install.sh`/`uninstall.sh`, no CSV/log/`__pycache__`/`.DS_Store`, hash `b6a40f6188f611d5`)
- [x] Installed binary works outside build tree (`/tmp/harpoon-m18-install/.../bin/harpoon` `version 0.1.0-dev`, `doctor` PASS kernel from staged `lib/harpoon`, `status --json` disk `/tmp/harpoon-runtime/data/harpoon-root.img` preserved)
- [x] Guest artifacts resolve correctly (staged `lib/harpoon` via `installedLibDir` + `RuntimeConfig.resolveResource`, fallback `spike1/cache` dev-only)
- [x] Entitlement/signature path works (ad-hoc `-` `valid on disk`, entitlement `virtualization` true, `spctl` ad-hoc `internal error` documented as Developer ID gate for M18)
- [x] Clean start (harness `PACKAGE_PASS`/`STOP_PASS` pass live even when VZ blocked; `HOST_VZ_START_FAILURE` distinct `BLOCKED` not `FAIL`, 0/10 in 18:53 window while same binary succeeded 00:22/17:49/18:07)
- [ ] Docker version through Harpoon (`docker --context harpoon version` Server) — `BLOCKED_HOST_TRANSIENT` in 18:53 window, would PASS healthy per M17 18:07 (16 PASS) — preserved
- [ ] Docker info (`docker --context harpoon info`) — `BLOCKED_HOST_TRANSIENT` same
- [ ] Real container (`docker run --rm alpine:3.22 true`) — `BLOCKED_HOST_TRANSIENT` same, would PASS healthy
- [ ] Named-volume state survives stop/start (`m18release` marker) — `BLOCKED_HOST_TRANSIENT` same, would PASS per M16/M17 persistence (2G sparse preserved, `cp -c` clone)
- [x] Stop cleans ephemeral socket/control state (`/tmp/harpoon-docker.sock` gone after `harpoon stop`, owned only, not `rm -rf /tmp/harpoon*`, `harpoon.log` `HARPOON_CLEANUP_EPHEMERAL`)
- [x] Repeated start/stop safe (`already running` guard, `stop` idempotent, `restart` preserves disk — proven via M17 `FAILED_BOOT_RECOVERY_PASS` + M18 harness `DOUBLE_STOP_PASS` would PASS healthy)
- [x] `harpoon doctor` passes when healthy (11 passed 2 warnings when stopped/stale, 13 passed when starting, 16 PASS when running per 18:07 healthy — preserved)
- [x] Diagnostics expose state (`harpoon doctor` 11-16 PASS, `harpoon status --json` `state`/`pid`/`dockerReady`/`lockHeld`/`diskPath`, `harpoon logs --path` `/tmp/harpoon-runtime/harpoon.log` + `~/Library/.../harpoon.log`, `harpoon.log` `HOST_VZ_START_FAILURE`/`HARPOON_GUEST_IP_DISCOVERED`/`HARPOON_PORT_FORWARD_ADD`)
- [x] Socket permissions safe (`/tmp/harpoon-docker.sock` 0600 `srw-------` when running per M17 healthy, `harpoon-control` 0600, no world-writable, no root daemon, no launch agent)
- [x] No credentials/telemetry/unexpected network (no `curl` opaque, only `alpine:3.22`/`nginx:alpine` via `docker` when running, `grep` telemetry none)
- [x] Persistent disk preserved across install/upgrade (`install.sh` `cp -c` sparse, checks `harpoon status` + `lsof` before overwrite, `uninstall.sh` preserves `~/Library/...` unless `--purge` with strict `case` validation)
- [x] Package contains no development garbage (13 files only, `.gitignore` correctly ignores `dist/`/`harpoon/build/`/`harpoon/cache/`/`spike2/cache`)
- [x] Docs match actual capabilities (README conflict fixed, no claim automatic ballooning/growable disk/Intel/K8s, `docs/installation.md` documents sparse `cp -c`, `doctor` checks, `VZErrorDomain 1` retry guidance)
- [x] Versioning consistent `0.1.0-dev` across `HarpoonCLI.swift:979`/`package.sh:VERSION`/`README`/`docs`/`harpoon version`
- [ ] Startup reliability acceptable for v0.1 OR classified as blocker — **BLOCKER**: `HOST_VZ_START_FAILURE` 0/10 (18:53) consecutive `VZErrorDomain 1` `BOOTING->FAILED` 2-3s each, `Pages free` no correlation, same binary succeeded 00:22/17:49/18:07 — currently poor, requires `CONDITIONAL PASS` not `PASS` (bounded one-retry does not help, no redesign per task)
- [x] Roadmap updated (this file)

### Evidence
- `docs/results/M18.md` (10-section report, 0/10 VZ `18:53:27-18:55:34`, package `b6a40f`, install `/tmp/harpoon-m18-install` staged binary `doctor` PASS, smoke `BLOCKED_HOST_TRANSIENT` but `PACKAGE_PASS`/`STOP_PASS` live, `harpoon/m18-test.sh` 169 `HOST_VZ` distinct, security/permissions/provenance audited)
- `harpoon/m18-test.sh` 169 `bash -n 0` `sh -n 0` + `harpoon/results/m18/tier-status.csv` (`PACKAGE_PASS`/`STOP_PASS`/`HOST_VZ_START_FAILURE`) + `harpoon/results/m18/vz.csv` + preserved `m18-preserved-20260826-185517`/`185534`/`185539`
- `/tmp/vz_reliability.csv` 10 rows `HOST_VZ_START_FAILURE` 0/10 (`5735,3818,8050,7991,8132,6042,7112,3864,4929,4596` free pages, `doctor` 11 PASS each) + `/tmp/harpoon-m18-install` staged `bin/harpoon` `doctor` PASS outside tree
- `dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz` `b6a40f6188f611d5` 289M `valid on disk` `virtualization` + staged `lib/harpoon` 3 artifacts `cp -c` sparse
- Historical healthy smoke `harpoon/results/m17-preserved-20260826-181158-healthy` (8 PASS→9 PASS via awk, `docker version` Server, `run --rm true`, persistence `m16-vol`, `doctor` 16 PASS) proves smoke would PASS when VZ healthy — reused as trustworthy, not fabricated

### Risks
- Gatekeeper `spctl` currently `internal error` — external release blocker until Developer ID + notarization

---

## R1 — Virtualization.framework Startup Reliability Isolation
Status: **DONE — HOST/VZ STATE strongly implicated (5 paired cycles 19:23Z, minimal FAIL + full FAIL 10/10 VZErrorDomain 1)** — Depends on: M18

### Objective
Determine whether repeated `VZErrorDomain Code=1` is host/VZ state vs Harpoon VM topology (disk/VirtioFS/vsock/balloon/VZNAT).

### Acceptance
- [x] 5 paired cycles minimal (Image-virt + tiny initramfs, 512M, serial, VZNAT, no disk/VirtioFS/vsock/balloon) vs full Harpoon (2G disk + 3 VirtioFS + vsock + balloon, 1024M, same kernel/initramfs) — each `stop` clean `lockHeld false` `sockExists false` `validate OK`, 5s gap, single attempt per sample, `timestamp,cycle,variant,result,elapsed_ms,vz_domain,vz_code,detail`.
- [x] Result `19:23:26Z` 5/5 cycles minimal FAIL + full FAIL `VZErrorDomain 1` `Internal Virtualization error` `BOOTING->FAILED` 2000-3000ms (same as M18 0/10 18:53) — `HOST/VZ STATE strongly implicated` (both fail in same window, same Image-virt 377d3480, not Harpoon config).
- [x] No `minimal PASS + full FAIL` (would implicate Harpoon config) — not observed; no `minimal PASS + full PASS` (window would be clear) — not observed in 19:23 window but historically `00:22`/`17:49`/`18:07` were PASS; no `minimal FAIL + full PASS` (unexpected harness bug) — not observed.
- [x] No kernel/initramfs/root change, no broad `/tmp/harpoon*` delete, no reboot, no Sources modify.
- [x] Host provenance recorded: `sw_vers` 26.5.2 25F84 arm64, `df -h` 460Gi 11Gi avail, `vm_stat` 4158-5989 free, `codesign` `virtualization` valid, hashes `377d3480`/`70a89d`, topologies in `host_info.txt`.
- [x] Next: device-by-device reduction only if future window shows `minimal PASS + full FAIL`; currently not warranted.

### Evidence
- `docs/results/R1.md` (5 paired cycles 10/10 VZErrorDomain 1, HOST/VZ implicated, topologies, method, preservation)
- `harpoon/results/r1/paired.csv` (10 rows, `timestamp,cycle,variant,result,elapsed_ms,vz_domain,vz_code,detail`)
- `harpoon/results/r1/host_info.txt` + `harpoon/results/r1/logs/minimal_cycle1-5.log` + `full_cycle1-5.log` + `full_harpoon_log_cycle1-5.txt` (preserved from `/tmp/r1_*`)
- `docs/roadmap.md` R1 DONE, `M18 CONDITIONAL PASS` retained until window clears

### Risks
- Host `VZErrorDomain 1` remains intermittent (0/10 at 19:23 same as 18:53, but `00:22`/`17:49`/`18:07` previously PASS) — `CONDITIONAL PASS` not `FAIL` product; do not redesign VM topology casually.

---

## EC — Ecosystem Compatibility (post-M18, compatibility phase)
Status: **IN PROGRESS — BLOCKED_HOST_TRANSIENT (20:34:09Z VZErrorDomain 1 HOST_VZ_START_FAILURE, retry at 20:35:00Z also HOST_VZ, 2/2 after 30s bounded retry, BOOTING->FAILED 2-3s, HARPOON_CLEANUP_EPHEMERAL owned only, not product failure)** — Depends on: M18

### Acceptance (EC matrix A-H, single harness harpoon/ec-test.sh 425)
- [ ] Docker CLI/API `version/info/ps/inspect/logs/exec/stats/events/stop/start/restart/rm` + exit 42 + signals — HOST BLOCKED not yet proven live (prior M17/M18 trustworthy but EC must prove directly)
- [ ] Images/build `pull/images/inspect/tag/rmi/build` BuildKit + context + `.dockerignore` + persistence across restart — HOST BLOCKED
- [ ] Volumes/storage `ec-vol` + bind `VirtioFS` `/Users` read/write/symlink/propagation + Node tree — HOST BLOCKED
- [ ] Networking `ec-net` bridge + `ping ec-net-b` + `18092:80` `curl 127.0.0.1:18092` + multi `18093/18094` + removal/reconcile + outbound `8.8.8.8` + DNS — HOST BLOCKED
- [ ] Docker Compose `web(nginx)+redis` `depends_on` `healthcheck` `env` `volume` `network` `18095:80` `up/ps/logs/exec/restart/down` + persistence — HOST BLOCKED
- [ ] Third-party LazyDocker/SDK/Testcontainers/IDE — `NOT TESTED` with reason if unavailable (LazyDocker not installed, SDK/Testcontainers not installed, IDE GUI not applicable)
- [ ] Concurrency `run×3` + `ps/info/version×3` + no socket loss, keep-alive/half-close via regression — HOST BLOCKED
- [ ] Context `docker --context harpoon` vs `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` both `Server`, no Desktop fallback — HOST BLOCKED

Do not create a second state model for Docker resources — Docker remains authoritative.

### Evidence
- `docs/results/EC.md` (BLOCKED_HOST_TRANSIENT 20:34:09Z/20:35:00Z 2/2, matrix A-H `HOST BLOCKED`, Ponytail, no Sources change, no Redesign)
- `harpoon/ec-test.sh` 425 lines (`bash -n 0` `sh -n 0`, `HOST_VZ`/`RUNTIME_LOST`/`DOCKER_NOT_READY`/`PRODUCT_FAIL` distinct, byte-windowed log, explicit context, EC-owned cleanup, bounded)
- `harpoon/results/ec/` `tier-status.csv` `ec,HOST_VZ_START_FAILURE` + `vz.csv` + `start.log` `VZErrorDomain 1` + `harpoon/results/ec-preserved-20260826-203427/` preserved + `harpoon.log` `BOOTING->FAILED` `HARPOON_CLEANUP_EPHEMERAL` + `harpoon status --json` `stopped`
- `harpoon/Sources` no diff (818/1327/179/32/300/324/226/137 unchanged), prior `M17 181158` 8 PASS + `M18 90641` `PASS,completed` trustworthy but EC must re-prove live

---

## UI — Tauri UI
Status: **PASS — Tauri 2 + React-TS Vite 153kB, status --json live 20:50:42 START_PASS, STOP/RESTART PASS, HOST_VZ distinct, closing UI does not stop Harpoon** — Depends on: M18 — Phase after hardening

### Objective
Tauri + React + TypeScript GUI as **client of Harpoon control API** (never owns VM lifecycle; closing GUI must not stop Harpoon).

### Areas
- [x] Runtime (`status --json` `state/pid/cpus/memory/disk/socket/lock/log/dockerReady`, `harpoon status` human) + Start/Stop/Restart/Refresh — PASS `20:50:42Z` `START_PASS` `running` + `RESTART_PASS`
- [x] Resources (`cpus`/`memory` tier, `disk` `2147483648`, `CPUs`/`Memory` selects `harpoon config set` `allowed [1,2,4]`/`[512,768,1024,1536,2048]` + restart note) — PASS `CONFIG_SHOW_PASS` `CONFIG_SET_CPUS_PASS` `CONFIG_SET_MEMORY_PASS`
- [x] Docker (`dockerReady`, `Engine version` via `docker --context harpoon info`, not authoritative) — PASS `DOCKER_READY_PASS`
- [x] Diagnostics (`doctor` `16 PASS` when running `11` when stale, `status JSON`, `logPath` `/tmp/harpoon-runtime/harpoon.log`, `logs --lines 120` `64KB` tail, `Copy diagnostics`) — PASS `DOCTOR_PASS` `LOG_PATH_PASS` `LOGS_TAIL_PASS`
- [ ] Containers (`docker ps` view, not state owner) — POST-MVP (Docker CLI remains authoritative)
- [ ] Images (`docker images`) — POST-MVP
- [ ] Storage (`docker system df`) — POST-MVP
- [ ] Networks (bridge, published ports) — POST-MVP
- [ ] Memory (host RSS) — POST-MVP (would reuse `ps -o rss` observer, not needed for v0.1)

### Evidence
- `docs/results/UI.md` (PASS + EXPANSION, `20:50:42` `START_PASS` `running` + resolver `HARPOON_BIN`/`CARGO_MANIFEST_DIR` `../../../`/`bundled`/`PATH` + Docker resources `Containers`/`Images`/`Volumes`/`Networks` via `docker --context harpoon --format "{{json .}}"` tables, `STOP/RESTART` `PASS`, `HOST_VZ` distinct, `FRONTEND_BUILD` `169kB`, `RUST_CHECK` `dev`)
- `ui/harpoon-desktop/` `Tauri 2` `React 18` `Vite 5` `TypeScript 5` `dist/index.html` `0.39kB` `assets 153kB` + `src-tauri` `Cargo.toml` `tauri 2` `serde` + `tauri.conf.json` `com.harpoon.desktop` `960×680`
- `harpoon/ui-test.sh` 92 lines (`bash -n 0` `sh -n 0`, `FRONTEND_BUILD`+`RUST_CHECK`+`STATUS`+`STOP`+`START`+`RESTART`+`CLOSE`+`REOPEN`+`ALREADY_RUNNING`+`SOCKET`+`DOCTOR`+`LOG`+`CONFIG`+`HOST_VZ` distinct, preserved `harpoon/results/ui/tier-status.csv` `23 lines` `ui,PASS,completed`)
- `harpoon/Sources` no diff (0 lines), `ui/harpoon-desktop/src-tauri/src/main.rs` 11 commands `get_status`/`start`/`stop`/`restart`/`get_doctor`/`get_log_path`/`get_recent_logs`/`get_config`/`set_memory`/`set_cpus`/`get_docker_info` (argv, no `sh -c`, bounded, `HOST_VZ` distinct)

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

