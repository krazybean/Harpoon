# Performance Methodology

Harpoon's key principle: consume resources because containers need them, not because Harpoon does.

This doc defines how M13 baseline was measured and how future M14-M18 will be compared.

## Host measurement (macOS-native, repaired)

- Harpoon PID from `/tmp/harpoon.pid` or `pgrep -f "harpoon.*run"`.
  - `ps -o rss= -p "$PID"` → Harpoon RSS KiB (scalar, numeric only)
  - `ps -o %cpu= -p "$PID"` → CPU %
  - `ps -M "$PID" | tail -n +2 | wc -l` → threads (fallback `ps -o thcount=` if available)
  - `lsof -p "$PID" | wc -l` → FDs
- VM/XPC via disk association (repaired):
  - `get_disk_path()` → current Harpoon root image (`/tmp/harpoon-runtime/data/harpoon-root.img` or `spike2/cache/harpoon-root.img` via `status --json`/`doctor`)
  - `find_vm_pid()` → `lsof -n | grep -F "$disk" | grep -i Virtualization | awk '{print $2}'` → associated `com.apple.Virtualization.VirtualMachine` PID (defensible, not sum of all). If ambiguous, record `VM_PID_UNAVAILABLE` / `VM_RSS_UNAVAILABLE`.
  - `ps -o rss= -p "$VM_PID"`, `ps -o %cpu=`, `ps -M`, `lsof -p` → VM RSS/CPU/threads/FDs
  - `combined RSS = Harpoon RSS + VM RSS` (empty if VM unavailable)
- `vm_stat` → Pages free, `memory_pressure` → host pressure.
- Never report configured VM memory (512/768/1024) as host resident.

Output (clean, machine-readable, no embedded `ps` headers):
`harpoon/results/m13/host.csv` columns: `timestamp,tier,label,harpoon_pid,harpoon_rss_kib,harpoon_cpu_pct,harpoon_threads,harpoon_fds,vm_pid,vm_rss_kib,vm_cpu_pct,vm_threads,vm_fds,combined_rss_kib,pages_free`
Repair run: `harpoon/results/m13/repair-host.csv` (same schema), authoritative for corrected idle/load per `docs/results/M13.md`.

## Guest measurement

Via `docker --context harpoon run --rm alpine:3.22`:
- `cat /proc/meminfo` (MemTotal/MemAvailable/MemFree)
- `free -m`
- `cat /proc/loadavg`
- `ps aux | wc -l`

Output: `harpoon/results/m13/guest.csv` columns `timestamp,tier,label,mem_total_kib,mem_available_kib,load1,load5,load15,process_count` and per-tag `guest-*.txt`. Repair: `repair-guest.csv`.

## Startup (C)

For each tier 512/768/1024, 5 clean `harpoon stop; harpoon start` iterations:
- `T0` = `date +%s.%N` before `harpoon start`
- `T1` = log `starting VM kernel`
- `T2` = `HARPOON_RUNNING`
- `T3` = socket `srw------- /tmp/harpoon-docker.sock` (0600)
- `T4` = `docker --context harpoon version` succeeds
Reported `min/median/mean/p95` per tier, raw in `harpoon/results/m13/startup.csv` (15 rows). `VZErrorDomain 1` → `BLOCKED_HOST_TRANSIENT`, not counted. Startup is authoritative from first live run (17.6/16.6 etc) preserved in `harpoon/results/m13/archive-20260825-*/`.

## Idle (D)

After Docker ready, settle 10s, then sample `host.csv`+`guest.csv` every 5s for 30s (6 samples) per tier. Median + range. Repair mode repeats only this section with corrected host metrics.

## Container latency (E)

`docker --context harpoon run --rm alpine:3.22 true` ×10 and `hello-world` ×5 warm (pre-pulled), wall-clock `date +%s.%N` diff. Raw in `container.csv`. Valid live means `512 ~0.446s, 768 ~0.451s, 1024 ~0.450s` preserved.

## Memory under load (F, repaired)

Idle → start named `m13-stress` with `python:3-alpine` `bytearray(128M)` touching each 4096 (`x[i]=1`) + `sleep 90` (guaranteed resident 60s+) → verify `docker inspect -f '{{.State.Running}}' == true` before each sample → load +5s, hold +10s, hold +30s → `rm -f m13-stress` → release immediate, after10, after30. Host+guest sampled each phase. If workload exits, record `MEMORY_WORKLOAD_FAILED`, do not treat as valid. Previous defect `No such container: m13-stress` (alpine without python, 60s gone before sample) is invalid — replaced. Do not change balloon targets. Raw in `repair-memory-load.csv` (authoritative for load/recovery) and `repair-host.csv`/`repair-guest.csv`.

## CPU (G)

`ps %cpu` idle vs `docker ps` loop vs short launches vs 128M stress. Watch port-sync wakeups. DOCUMENT only.

## Compose (H)

`harpoon/fixtures/m9-compose` (`app:3000->18080`, `postgres:5432->18081`, `redis`, `worker`, `mem_limit 64m`): `compose up -d --build` wall time (512 ~5s, 768 ~4s, 1024 ~4s), host RSS before/after, `MemAvailable`, CPU, container count. Run at each tier; 512 may be `RESOURCE_EXHAUSTED`.

## Build (I)

`docker build -t m13-test:warm harpoon/fixtures/m9-compose` and `build --no-cache`, duration + peak host memory. All tiers ~1s at harness resolution.

## Filesystem (J)

Bind-mount `/tmp/m13-fs-test:/data` read/write, `ls -R`. Functional at all tiers, comparative only.

## Persistence (K)

`volume create m13-vol` marker → `harpoon stop` → `harpoon start` → `volume inspect` + `cat /data/marker` must survive. 512 PASS, 768 PASS, 1024 `WARN/BLOCKED` needs characterization (see M13). Harness now verifies `docker inspect` running, `volume inspect` success, and marker existence separately to distinguish timeout vs data loss.

## Soak (M)

Bounded 10m: every 30s `docker ps` + short `run` + `host`+`guest`. Look for monotonic growth only.

## Raw data versioning

- First live full run archived to `harpoon/results/m13/archive-20260825-211218/` (startup, container, compose, build, filesystem valid).
- Repair run stores `repair-host.csv`, `repair-guest.csv`, `repair-memory-load.csv`, `repair-persist-*.log` separately. `docs/results/M13.md` states which dataset is authoritative per section.
- Do not commit transient Docker artifacts.

## Limitations

Host `VZErrorDomain 1` may block live — `BLOCKED_HOST_TRANSIENT`. 512 may OOM. Do not reboot host or SSH guest.

## M14 Idle Optimization (PortForwardManager)

M14 changes `PortForwardManager.startPolling()` from `repeating: 2` (30/min) to `repeating: 10` (6/min, +5s initial). Before logs show `HARPOON_PORT_SYNC_START` every 2s even with `containers=0 mappings=0`; after shows every 10s (05:46:39/49 live). Correctness preserved via `scheduleSync` on `setGuestIP`/container events (fallback only, reconciles within 10s, dev tolerates). Harness `harpoon/m14-test.sh` now queries `logs --path` + newest mtime (was stale `/tmp/...` → 0/min). No transport/socket/filesystem/disk change. See `docs/results/M14.md` and `harpoon/Sources/PortForwardManager.swift:44-50`.

## M15 Memory Reclamation (Host-Visible) — PASS 2026-08-26

M15 audit (2026-08-26): `VZVirtioTraditionalMemoryBalloonDevice` + `/tmp/harpoon-control` + `virtio_balloon.ko` PROVEN, no automatic policy, no `drop_caches`, no pressure handling. Spike5 baseline `386→919 MB` demand-backed, `919` retained +60s, balloon guest `815→555→806 MiB` PASS but host `507→604→627 MB` **NOT OBSERVED**. Baseline harness `harpoon/m15-test.sh` 352 / `harpoon/m15-balloon-test.sh` 276 (both `bash -n 0` `sh -n 0`, pinned `wait_for_stable_runtime` + `verify_pinned_runtime` at 8+ seams, strict `combined=Harpoon+VM` via `ps`+`lsof`).

**Valid paired result (authoritative, preserved):** `512` idle `103872` → `release-60` `103568` (−304), `768` `389056` → `96816` (−292240), `1024` `374544` → `99696` (−274848); transient `189568/200448/217360` at `release-immediate` falls `~50%` by `+60s` at every tier — natural VZ reclamation **PROVEN host-visible**, no retention above `idle` (512 at parity, 768/1024 well below). Configured 512/768/1024 does not imply equivalent idle RSS (`103872` vs `389056` vs `374544`).

Balloon one-shot (`echo $target | nc -U /tmp/harpoon-control` single bounded, 50M headroom, no `drop_caches`) at `release-30/60`: `512` natural `126960/103568` vs balloon `101568/116960` and rerun `123200/114032`; `768` natural `97760/96816` vs balloon `116592/102592` and rerun `110000/94288` — **no consistent material win** (>15% at both +30/+60). Later `RUNTIME_LOST`/`syntax-broken` runs are archived invalid, not acceptance (`harpoon/results/m15-preserved-20260826-151813/host.csv` and `m15-balloon-preserved-20260826-152533/host.csv` + rerun `harpoon/results/m15-balloon/host.csv` are authoritative).

**Ponytail review (YAGNI):** 1) Need automatic balloon at all? No — natural reclamation already returns transient memory by +60s at all tiers. 2) Already exists? Manual balloon exists as EXPERIMENTAL, automatic does not and is not needed. 3-5) Stdlib/platform/dependency? `Virtualization.framework` already reclaims demand-backed memory. 6) One line? `echo $target | nc -U` already exists for manual. 7) Minimum code is *no* automatic policy — `ponytail: no automatic ballooning — YAGNI, manual control sufficient; revisit only if future host pressure proves retention >60s with quantified host RSS.` Keep balloon as EXPERIMENTAL/manual, do not add `drop_caches`, hysteresis, pressure policy, resize, or M16 work.

**M15: PASS — natural host-visible reclamation proven; balloon optimization rejected as unnecessary based on measurements.** No runtime code change.
