# SPIKE 5 — Memory Footprint, Ballooning, and Reclamation — PASS

Date: 2026-08-26. Host `Virtualization 1112.1.16` `HV OK` `isSupported true` `VZNAT` `VirtioFS 18.4.0` `phys_footprint` `footprint -p` Mac 11 vCPU Docker Desktop 4.49.0 8GiB configured vs Harpoon 1024 MiB 2 vCPU (caveat prominent).

## Outcome

SPIKE 5 PASS. Low-memory advantage demonstrated as development-time measurement with prominent caveat. COMPLETING SPIKES 1–5 — FEASIBILITY PHASE CLOSED (feasibility complete, not production readiness).

Harpoon idle ~386 MB footprints vs Docker Desktop idle 954 MB (2.5x). Workload idle 386→919 MB Harpoon vs 954→1757 MB Docker Desktop (1.9x under equivalent nginx+Redis+Postgres). Demand-backed growth proven (not preallocated), high-water NOT reclaimed post-workload, balloon guest reclaim PASS but host footprint reduction NOT OBSERVED on tested platform. Not universal guarantees.

## Configured vs Host vs Guest

1024 `HARPOON_MEMORY_MIB=1024` `HARPOON_MEMORY_CONFIG_MIB 1024` `HARPOON_MEMORY_CONFIG_BYTES 1073741824` `HARPOON_BALLOON_TARGET_INITIAL 1073741824` `HARPOON_BALLOON_RUNTIME_FOUND` guest `MemTotal ~970 MiB` `MemAvailable ~818 MiB` idle `VM XPC ~360-386 MB` (386 fresh 361)
768 `HARPOON_MEMORY_MIB=768` guest `MemTotal ~718 MiB` `MemAvailable ~565 MiB` idle `~367 MB` — no footprint win vs 1024
512 `HARPOON_MEMORY_MIB=512` guest `MemTotal ~468 MiB` `MemAvailable ~321 MiB` idle `~350 MB` — only 10-18 MB win for -502 MB configured
1024 sensible default; 512 functional save only.

## Behavior

Demand-backed: `386→532→643→919 MB` under workload (baseline+nginx+redis+postgres), host grows with demand not upfront. High-water: `919 MB +5s 919 +30s 919 +60s 919` NO natural drop — high-water retained.

## Balloon

Device `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` `vm.memoryBalloonDevices = [balloon]` `targetVirtualMachineMemorySize mutable` `vm.memoryBalloonDevices[0].targetVirtualMachineMemorySize = UInt64(target)` `failable`.

Guest `virtio_balloon.ko` 36K kernel `6.12.94-0-virt` `virtio_balloon virtio_balloon.* device is virtio` `HARPOON_BALLOON_DRIVER_OK` `decompress 8.7M decompressed size` `virtio_net virtio_blk fuse` builtins.

Control `/tmp/harpoon-control` Unix `0600` `HARPOON_BALLOON_CONTROL_LISTENING` file+vsock full-duplex retained read (stdin fix snap280 handshake desync `bridges nil`→hang), per-connection `uint16+metric+name+fifo-reader-length-buffered` `DispatchSource.makeReadSource` retained `resume`, client `echo 768 | nc -U /tmp/harpoon-control`, host markers `HARPOON_BALLOON_TARGET_REQUEST 805306368 HARPOON_BALLOON_TARGET_SET 805306368 HARPOON_BALLOON_TARGET_APPLIED 805306368` `HARPOON_BALLOON_TARGET_REQUEST/SET/APPLIED` ground truth, early `header 00153` filtered.

Results `MemAvailable 815→555→806 MiB` `220→480→229` balloon `BALLOON_GUEST_RECLAIM PASS`, host `HARPOON_VM_PHYS_FOOTPRINT_MB` sampled `507→604→627 MB` continuous `PHYS_FOOTPRINT` from balloon-issued time `BALLOON_HOST_FOOTPRINT_REDUCTION NOT OBSERVED` (507 before 604 after inflation 627) — device cooperation not `phys_footprint` pressure relief on tested platform/pressurized host — DO NOT market host reclamation as proven.

## Workload Equivalence

Docker Desktop `4.49.0` `linux/arm64` `28.3.3` `11 CPUs ~7.9 GiB` `docker run -d --name nginx` `redis 467M` `postgres:16-alpine POSTGRES_PASSWORD 467M`; Harpoon `--dns 1.1.1.1 8.8.8.8 --network-alias` compat DNS fix (macOS NAT `no host DNS` vs Desktop bridge `192.168.65.7`), `virtio_net` `virtio_balloon` `fuse/virtiofs`, `/run tmpfs` lesson preserved.

## Benchmark Qualified Claim

On the development test system with Harpoon at 2 vCPU 1024 MiB and Docker Desktop at 11 vCPUs ~8 GiB, Harpoon used approximately 2.5x less physical memory at idle (~386 vs ~954 MB) and approximately 1.9x less under the same nginx+Redis+PostgreSQL workload (~919 vs ~1757 MB). WINDOW `+5s stable` `+30s/+60s high-water no reclamation` SAME-DAY same-Mac VM XPC vs Docker VM process, `nginx+Redis+Postgres 467M each` equivalent, `compat --dns` — NOT normalized, no universal guarantees, future measurements expected to vary — `ponytail: direct win, scaling and cold-start latent`.

## Lessons

Persistent `ext4` `/run tmpfs` `containerd dockerd reuse` stale `dockerd.sock` `PID` cleanup, `VirtioFS` host `VirtioFS` guest `fuse/virtiofs` modules + `--dns` for `postgres` `redis` lookup, balloon full-duplex buffered control, compressed kernel `IMAGE_DECOMPRESSED` path probe, `0700` `extracted` permission isolation.

## Future Work

Memory policy engine `harpoon-memory` spike2 does NOT include (`harpoon-vz` integration spike only); control snapshot method not live pressure policy.

## Artifacts

`spike2/build/harpoon-spike2-vsock` 243K `5a68b30a...` `spike2/cache/harpoon-docker-initramfs.cpio.gz` decompressed size `622ae1ef...` `harpoon-root.img 2G sparse`, `HARPOON_MEMORY_CONFIG_MIB/BYTES` `HARPOON_BALLOON_*` markers, `/tmp/harpoon-control` `HARPOON_BOOT_*` `HARPOON_VIRTIOFS_RW_OK` logs, `communication-swift.md` multi-read audit addressed.
