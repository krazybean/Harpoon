# MVP Definition — Harpoon 0.1

Scope: **ordinary local Docker development on Apple Silicon with substantially better memory behavior.**

Not a Docker Desktop clone.

## Must for MVP

- [ ] `harpoon start/stop/restart/status/doctor/memory` works
- [ ] `VZVirtualization.framework` VM boots ARM64 Linux (Spike 1)
- [ ] Docker API at `~/.harpoon/docker.sock` with `docker context` (Spike 2)
- [ ] `docker run --rm hello-world`, `pull`, `build`, `volume`, `network`, `exec/logs/inspect/ps` pass (requirements.md acceptance 1-11)
- [ ] Bind mounts via VirtioFS with correctness for dev workflows (Spike 4)
- [ ] Port publishing `docker run -p` → `localhost:<port>` reachable (Spike 3)
- [ ] VM disk persists across restarts (R7)
- [ ] Outbound internet from containers
- [ ] Docker bridge + user-defined networks
- [ ] Host↔guest clock sync (VM clock drift after sleep/pause)
- [ ] Graceful shutdown / crash recovery / stale socket cleanup
- [ ] Signing & `com.apple.security.virtualization` entitlement for distribution
- [ ] Guest kernel provenance + version pin + checksum, reproducible acquisition, no opaque binaries in repo

## Should for MVP

- Compose `docker compose up -d` (acceptance #8)
- LazyDocker / Testcontainers compatibility (best-effort, verified not merely claimed)
- BuildKit support (via Docker Engine)
- ARM64/x86 image behavior documented (x86 emulation not required, but `PLATFORM` error must be clear)
- Logs & diagnostics under `~/.harpoon/log` or similar
- `harpoon doctor` checks for virtualization support, entitlements, disk, socket stale

## Explicit Post-MVP

- Kubernetes (Easter egg only), multiple VMs, arbitrary Linux distros, Intel Macs, Swarm, Windows containers, USB passthrough, production cluster, extension marketplace, cloud orchestration, auto-HTTPS, complex DNS/VPN

## Open Questions → classify quickly

- Private registries + credential helpers + CA certificates: SHOULD (affects many corp users) but defer until Spike 2 proves transport, then validate `docker pull` via private registry.
- Proxy / VPN behavior: POST-MVP unless trivial via VZNAT pass-through; spike networking may reveal.
- DNS / IPv6 inside VM: MUST minimal (IPv4 outbound + Docker embedded DNS), IPv6 POST-MVP
- Bind-mount UID/GID mapping details: MUST define (see compatibility.md)
- File watching specifics: MUST verify inotify propagation (Spike 4), performance POST-MVP
- Sleep/wake, host network changes, host reboot: MUST handle cleanly (daemon restart, VM save/restore or reboot)
- Disk format / growth / reclamation: MUST (qemu-img raw/qcow2? APFS sparse?); POST-MVP shrink optimization but MUST not leak unbounded growth.
- Installation / upgrades / rollback / config migration: SHOULD for MVP (manual `brew` or script okay; not silent auto-update)

## Acceptance Gates

MVP is done only when memory acceptance test passes on host-resident measurement, not just guest stats.
