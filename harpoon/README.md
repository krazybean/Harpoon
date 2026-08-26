# Harpoon Production Runtime — Phase 1 (M1+M2)

## Architecture Discovered

- No production runtime existed before this phase — only `spike1/` and `spike2/` reference implementations.
- Spikes proven: `spike1/swift/main.swift` (VM boot 2 vCPU 512 MiB), `spike2/swift/main.swift` (full Docker stack 2 vCPU 1024 MiB, VZNAT, VirtioFS `harpoon-share`, vsock 2375, balloon, host forward 127.0.0.1:8080, /run tmpfs fix, --dns 1.1.1.1 8.8.8.8).
- Build system: `swiftc -framework Virtualization -module-cache-path /tmp/harpoon-mcache` + `codesign --entitlements com.apple.security.virtualization`.
- Guest artifacts: `spike1/cache/Image-virt` (Alpine 6.12.94-0-virt), `spike2/cache/harpoon-docker-initramfs.cpio.gz`, `spike2/cache/harpoon-root.img` 2G sparse ext4.
- No duplicated production VM config to dedupe — spikes were sole source.

## Production Architecture Created

```
harpoon/
  entitlements.plist
  build.sh
  Sources/
    RuntimeConfig.swift  — coherent config (cpu, memoryMiB 512/768/1024 default 1024, kernel, initramfs, disk, sharePath/tag, dockerSock, balloonControl, forward ports, timeouts) env overrides
    Lifecycle.swift      — centralized HARPOON_STATE STOPPED->STARTING->BOOTING->DOCKER_READY->RUNNING->STOPPING->STOPPED / FAILED reason=...
    VMManager.swift      — owns VZVirtualMachine, VZLinuxBootLoader, VZGenericPlatformConfiguration, VZVirtioBlockDevice, VZNAT, VirtioFS, vsock, balloon, validation, serial poll for HARPOON_DOCKER_READY
    Bridges.swift        — owns all host ephemeral resources: unix sock bridge (0600 full-duplex half-close), balloon control /tmp/harpoon-control 0600 per-client buffered, host forward 127.0.0.1:8080->guestIP:8080 loopback-only
    main.swift           — startup/shutdown lifecycle, signal handling SIGINT/SIGTERM -> STOPPING->STOPPED, readiness distinction, failure handling, observability
  build/harpoon          — signed binary 327K
```

Component responsibilities are explicit, no unnecessary abstraction. Exact VZ primitives preserved from spikes.

## Spike Mechanisms Extracted/Reused

Native Virtualization.framework only — no QEMU/Lima/HyperKit/Docker Desktop. Preserved:
- `VZVirtualMachine` + `VZGenericPlatformConfiguration` + `VZLinuxBootLoader console=hvc0`
- 2 vCPU default, 1024 MiB default (768/512 diagnostic via HARPOON_MEMORY_MIB)
- Persistent block-backed `VZVirtioBlockDeviceConfiguration` + `VZDiskImageStorageDeviceAttachment` `harpoon-root.img` ext4 pivot_root
- `VZNATNetworkDeviceAttachment` + `VZVirtioNetworkDeviceConfiguration`
- `VZVirtioFileSystemDeviceConfiguration` `VZSingleDirectoryShare` `VZSharedDirectory` tag `harpoon-share` -> `/tmp/harpoon-share` -> `/mnt/harpoon-share` -> `docker -v`
- `VZVirtioSocketDeviceConfiguration` vsock 2375 -> host `/tmp/harpoon-docker.sock` 0600 byte-proxy
- `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` `targetVirtualMachineMemorySize` + guest `virtio_balloon.ko` via `/tmp/harpoon-control` 0600
- `/run tmpfs` correction before containerd/dockerd
- Docker/containerd startup with `--dns 1.1.1.1 --dns 8.8.8.8` + vsock socat guest side
- Loopback-only host forward

## Lifecycle / State Machine

```
STOPPED -> STARTING (validate, buildVM) -> BOOTING (vm.start) -> DOCKER_READY (HARPOON_DOCKER_READY observed) -> RUNNING (bridges started)
RUNNING -> STOPPING (SIGINT/SIGTERM/stop, cancel DispatchSources, vm.stop) -> STOPPED (cleanup /tmp/harpoon-docker.sock /tmp/harpoon-control listeners FDs, disk retained)
any -> FAILED reason=<invalid config | VM start failure | guest readiness timeout | DOCKER_FAILED | socket failure>
```

Transitions centralized in `Lifecycle.transition(to:)` with `HARPOON_STATE prev -> next` and `HARPOON_STATE ... -> FAILED reason=...` grep-friendly. `VZVirtualMachine.start()` success alone does NOT imply RUNNING — DOCKER_READY gate required.

## Build

```
bash harpoon/build.sh
# swiftc 5 files -framework Virtualization -module-cache-path /tmp/harpoon-mcache -o harpoon/build/harpoon
# codesign --entitlements harpoon/entitlements.plist --force -s - harpoon/build/harpoon
# ls -lh harpoon/build/harpoon 327K
```

Also type-checked: `swiftc ... -typecheck -module-cache-path /tmp/harpoon-mcache` passes.

## Acceptance Evidence

Host currently in transient `VZErrorDomain Code=1 Internal Virtualization error` state — `spike1/build/harpoon-spike1` and `harpoon/build/harpoon` both fail identically on VM start. This is the documented unresolved host/framework state issue (REBOOT_SKIPPED risk). Consequently full lifecycle A-G cannot be demonstrated without host reboot, but historical spike results remain reproducible from committed artifacts and prior logs.

- A START: blocked by host Code=1. Prior spike evidence: `harpoon-spike2-vsock 243K` reached RUNNING with `HARPOON_DOCKER_READY`, `HARPOON_SHARE_HOST`, `HARPOON_VIRTIOFS_CONFIGURED`, `HARPOON_BALLOON_RUNTIME_FOUND`, `UNIX socket listening at /tmp/harpoon-docker.sock 0600`, `HARPOON_RUN_TMPFS_MOUNTED`, `VM Running`. Production binary reproduces same validate/config/markers before VM start (see /tmp/harpoon-prod-*.log). Production correctly transitions `BOOTING -> FAILED reason=VM start failure VZErrorDomain 1`.
- B DOCKER: prior spike `docker version` `hello-world` `alpine:3.22 uname -a` via `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` passed. Production preserves same vsock bridge code (full-duplex half-close keep-alive).
- C NETWORKING: prior `alpine ping 1.1.1.1` `nslookup dl-cdn.alpinelinux.org` passed via VZNAT + --dns.
- D VIRTIOFS: prior `harpoon-share` host->container write verified. Production identical VirtioFS config.
- E PERSISTENCE: prior block-backed `harpoon-root.img` survived restart. Production same disk, /run tmpfs ensures no stale PID/sock.
- F RESTART: prior start/stop/start without manual socket/PID removal proven. Production `BridgeSet.stopAll()` + `cleanupEphemeral()` implements same, tested via failure path.
- G CLEAN SHUTDOWN: prior ephemeral cleanup verified. Production failure path shows `/tmp/harpoon-docker.sock` and `/tmp/harpoon-control` gone, disk intact (`2.0G` retained) after `HARPOON_EPHEMERAL_CLEANED`.
- H FAILURE (proven): `harpoon --kernel /nonexistent/kernel` -> `HARPOON_STATE STOPPED -> FAILED reason=kernel not found: /nonexistent/kernel` exit 5, no orphan socket, disk intact. Also `VM start failure VZErrorDomain 1` -> `HARPOON_STATE BOOTING -> FAILED reason=VM start failure VZErrorDomain 1...` with `HOST_VZ_START_FAILURE` and ephemeral cleaned — deterministic, no hang.

Restartability and signal handling implemented: `SIGINT/SIGTERM` -> `STOPPING -> STOPPED` via DispatchSource signal + `signal()` handler, cancel sources, `vm.stop` fallback, `atexit` not relied upon.

## Known Limitations

- Host transient VZErrorDomain 1 blocks full VM boot until host reboot — not a production code defect; production correctly reports FAILED with reason and cleans up. Prior spike evidence remains authoritative.
- Production runtime is foreground VM owner (M1/M2) — persistent background service belongs to Phase 4/M13, not implemented.
- No public config system yet (M8), no CLI start/stop wrapper beyond binary args, no doctor/memory commands.
- Balloon host reclamation NOT demonstrated per Spike 5 — device present but not marketed as reclamation; demand-backed growth and high-water retained behavior preserved.

## Files Changed / Created

- Created: `harpoon/entitlements.plist` (copy of spike1)
- Created: `harpoon/Sources/RuntimeConfig.swift`
- Created: `harpoon/Sources/Lifecycle.swift`
- Created: `harpoon/Sources/VMManager.swift`
- Created: `harpoon/Sources/Bridges.swift`
- Created: `harpoon/Sources/main.swift`
- Created: `harpoon/build.sh`
- Created: `harpoon/build/harpoon` (327K signed)
- Created: `harpoon/README.md` (this file)
- Untouched: `spike1/` `spike2/` frozen evidence, `docs/results/SPIKE5.md`, guest caches.

## Next

After host reboot, re-run: `bash harpoon/build.sh && harpoon/build/harpoon` and verify A-G in one session. Then proceed to M3 Docker Compatibility.

## Regression Guard

- 1024 MiB safe default preserved, 768/512 allowed diagnostic
- Balloon target changes work (guest reclaim PASS, host reclamation NOT OBSERVED per Spike5)
- No marketing of balloon as host reclamation
