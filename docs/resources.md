# Resources — CPU / Memory / Disk / Balloon

Production resource model for Harpoon Phase 1 M6. See `docs/architecture.md` and `docs/memory-model.md`.

## CPU

- Default `2 vCPU`, `RuntimeConfig.cpuCount=2`, `VZVirtualMachineConfiguration.cpuCount`.
- CLI `harpoon --cpus 2` (alias `--cpu`), env fallback `HARPOON_CPUS`, precedence `CLI > env > default`.
- Validation `1...8` (conservative Phase 1, framework maximum may be larger but 8 is safe). <1 or >8 → `HARPOON_STATE STOPPED->FAILED` `cpuCount must be 1...8`, exit 5, no socket, no orphan. Logs `HARPOON_CPU_CONFIG_COUNT <n>` and `HARPOON_BALLOON_CONFIGURED ... cpu=<n>`.
- Tested 1 and 2 CPU (M6 harness `--stage 1024` includes CPU sanity; higher counts characterized not optimized).

## Memory

- Default `1024 MiB`, tiers `512/768/1024` as production (not diagnostic-only). `RuntimeConfig.memoryMIB`, `memorySizeBytes = MiB*1M`.
- CLI `harpoon --memory 1024|768|512`, env `HARPOON_MEMORY_MIB`, precedence `CLI > env > default`.
- Validation: must be exactly `512/768/1024`, otherwise `HARPOON_STATE FAILED` `memoryMIB must be 512/768/1024`. Env invalid `HARPOON_MEMORY_MIB=999` warns `HARPOON_MEMORY_CONFIG_WARN` and clamps to 1024; CLI invalid `--memory 128` fails (no silent fallback).
- Logs `HARPOON_MEMORY_CONFIG_MIB`, `HARPOON_MEMORY_CONFIG_BYTES`, `HARPOON_RESOURCE_CONFIG cpus=.. memoryMiB=.. disk=..`.
- Guest observed `MemTotal`: `1024→~970`, `768→~718`, `512→~468` (via `grep MemTotal /proc/meminfo`). Idle host `phys_footprint` via `footprint -p <pid>`: `1024 ~360-386`, `768 ~367`, `512 ~350` (demand-backed, not preallocated). See `docs/results/SPIKE5.md` caveats.

## Disk

- Image `spike2/cache/harpoon-root.img` sparse raw ext4 logical `2 GiB` (`2147483648 bytes`), `Docker Root Dir /var/lib/docker` (bind mounts not counted).
- `RuntimeConfig.diskURL`, `diskLogicalBytes`, CLI `--disk PATH`, env `HARPOON_DISK`.
- Startup logs `HARPOON_DISK_IMAGE <path>` `HARPOON_DISK_LOGICAL_BYTES <n>` `HARPOON_RESOURCE_CONFIG ...`. Guest `df -B1 /var/lib/docker` and `docker system df` characterize used/available; warning threshold `<10% or <256 MiB` → `HARPOON_DISK_LOW_SPACE` (observability only, no auto-prune).
- Policy: default `2 GiB` fixed for M6; `harpoon --disk 4G` growth is future work, must never truncate existing disk, never shrink, never destroy data, grow ext4 safely and prove live. Sparse host allocation, not preallocated.

## Balloon

- Device `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` `virtio_balloon.ko`, control `Unix /tmp/harpoon-control mode 0600` (`HARPOON_BALLOON_CONTROL_LISTENING`).
- Logs `HARPOON_BALLOON_RUNTIME_FOUND`, `HARPOON_BALLOON_TARGET_INITIAL`, `HARPOON_BALLOON_TARGET_REQUEST/SET/APPLIED`, `HARPOON_BALLOON_TARGET_REJECT`.
- Truthful semantics: guest `MemAvailable` reclaim `815→555→806` `BALLOON_GUEST_RECLAIM PASS` but `HOST_FOOTPRINT_REDUCTION NOT OBSERVED` (`507→604→627 MB`) on `macOS 26 VZ 1112.1.16` — do NOT market as host reclamation, retained for compatibility/experimentation/future pressure.
- Safety bounds (M6): floor `512 MiB`, cap `configured VM memory`, only tiers `512/768/1024` ≤ configured. For `512` only `512` allowed, `768` allows `512/768`, `1024` allows `512/768/1024`. Arbitrary `600` or `128` → `HARPOON_BALLOON_TARGET_REJECT ... reason=...` no crash.

## Separation

- Harpoon owns VM envelope `cpu/memory/disk/balloon`. Docker owns per-container ` --memory 64m`, `cpu quota`, cgroups (`docker inspect --format '{{.HostConfig.Memory}}'` preserved). No rewriting of Docker limits.

## Precedence

`CLI > environment > defaults`. `--help` shows defaults and precedence.

## Limitations (M6)

- `512` lightweight tier functional but constrained; document as minimum.
- Host `phys_footprint` demand-backed `~386→919 MB` workload, high-water retained `919` not reclaimed post-workload (platform behavior, not fixed formula).
- Disk auto-grow deferred, balloon host reclamation not proven, UDP/IPv6/Compose out of scope for M6.

## Benchmark Caveat (retained)

Same dev Mac `2 vCPU 1024 MiB` vs Docker Desktop `11 vCPU ~8 GiB` (`~386 vs 954 idle 2.5x`, `~919 vs 1757 workload 1.9x` `nginx+Redis+Postgres` equiv `VirtioFS` `+18 MB` noise, `--dns 1.1.1.1 8.8.8.8` compat, demand-backed `386→919`). Not normalized, not universal, not bare-metal.
