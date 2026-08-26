# Requirements

This document defines Harpoon v0.1 scope: platform targets, functional requirements,
acceptance tests, and explicit non-goals. Architecture is described in
[architecture.md](architecture.md); memory behavior in [memory-model.md](memory-model.md).

## Platform Scope (v0.1)

- macOS
- Apple Silicon
- ARM64 Linux guests
- Docker Engine compatibility
- Docker Compose compatibility
- Local developer workloads

Intel Mac support is not required for the initial MVP. x86_64 container emulation is
not required for the initial MVP unless it proves inexpensive to support through
existing platform primitives.

## Functional Requirements

### R1 — Docker API compatibility (first-class interface)

Harpoon exposes a Docker-compatible Unix socket at `~/.harpoon/docker.sock`, backed by
the Linux Docker daemon. A Docker context must be able to target this socket.

- Host socket traffic is proxied into the guest over a deliberate host-to-guest
  transport such as virtio sockets.
- Must not depend on exposing `/var/run/docker.sock` through a shared filesystem.
- Compatibility extends beyond the Docker CLI. Expected clients:
  - Docker CLI
  - Docker Compose
  - LazyDocker
  - IDE integrations
  - Testcontainers
  - Scripts using the Docker API
  - Other Docker-compatible developer tooling

### R2 — Images

Normal Docker image workflows work through Docker Engine, using BuildKit rather than a
Harpoon-specific builder. Required MVP behavior:

```bash
docker pull
docker build
docker images
docker tag
docker inspect
docker rmi
```

Existing Dockerfiles must work without Harpoon-specific modifications.

### R3 — Containers

Required MVP behavior:

```bash
docker run
docker create
docker start
docker stop
docker restart
docker rm
docker exec
docker logs
docker inspect
docker ps
```

Harpoon introduces no container abstraction of its own; Docker remains authoritative
for container state.

### R4 — Storage

Both Docker-managed persistent volumes and macOS bind mounts are required.

- **Named volumes** reside on Linux-native persistent VM storage
  (`/var/lib/docker/volumes/`). The underlying VM disk persists across Harpoon restarts.
- **Bind mounts**: macOS directories are mountable into Linux containers via
  VirtioFS (`macOS dir → VirtioFS → guest → Docker bind mount → container`).
- No custom filesystem caching in the first implementation unless required for
  correctness; measure filesystem performance before optimizing it.

### R5 — Networking

Initial networking favors correctness and simplicity. Docker Engine owns bridge
networking, container-to-container communication, user-defined networks, aliases, and
internal DNS. Harpoon owns only the VM⇄macOS boundary.

Required MVP behavior:

- Outbound container Internet access
- Host-to-container published ports
- `localhost:<published-port>` access from macOS
- Docker bridge networks
- User-created Docker networks

Advanced custom DNS names, automatic HTTPS, and specialized VPN behavior are outside
MVP scope.

### R6 — Dynamic memory management

The VM grows into available capacity under load and relinquishes unused guest memory
afterward, using Apple's supported memory ballooning primitives where appropriate.
Detailed behavior, signals, and acceptance criteria are specified in
[memory-model.md](memory-model.md).

### R7 — Runtime persistence

Stopping Harpoon must not destroy the developer environment. After `harpoon stop`
followed by `harpoon start`, the following persist as Docker semantics normally
require:

- Downloaded images
- Named volumes
- Stopped containers
- Docker metadata
- Network definitions
- Build cache where appropriate

### R8 — Daemon independence

`harpoond` operates independently of all clients (CLI, GUI, diagnostics). Closing any
client must not stop Harpoon, the VM, or running containers. Frontend concerns must not
be embedded in the daemon.

### R9 — CLI surface

```bash
harpoon start
harpoon stop
harpoon restart
harpoon status
harpoon doctor
harpoon memory
```

Additional commands are added only when they operate on Harpoon-level concerns. No
aliases for existing Docker commands.


## Spike Plan (recorded here for traceability)

Spikes prove seams before broad implementation:

- **Spike 1 — Virtualization foundation** (this task): boot ARM64 Linux via Virtualization.framework, prove guest init via serial, clean shutdown, error reporting. See `spike1/` for implementation.
- **Spike 2 — Docker API bridge:** `docker version/info/run hello-world` via Harpoon socket.
- **Spike 3 — Networking:** `docker run -p 8080:80` → `http://localhost:8080` reachable.
- **Spike 4 — Bind mounts:** VirtioFS correctness for dev workflows.
- **Spike 5 — Memory reclamation:** host `phys_footprint` decrease after workload.

Each spike must produce host-observable evidence, not guest-only stats.

## Missing Requirements Audit (MUST/SHOULD/POST-MVP/OPEN)

| Area | Classification | Note |
|------|---------------|------|
| VM bootstrapping (kernel+initramfs direct boot) | MUST | Alpine virt kernel provenance pinned |
| Guest kernel provenance / updates | MUST (pin) / SHOULD (update mechanism) | Doc source+sha256, no opaque blobs |
| Rootfs construction | MUST minimal appliance | Alpine/minimal, no desktop |
| VM disk format (raw/qcow2, sparse) | MUST define | APFS sparse handling |
| Disk growth / reclamation | MUST not leak / POST-MVP shrink | Monitor `~/.harpoon/vm/disk.img` |
| Crash recovery / daemon restart / stale sockets | MUST | `harpoon doctor` + trap |
| Guest agent protocol (vsock control channel) | MUST define (Spike 2) | Health/memory telemetry |
| Startup / shutdown ordering | MUST | agent → Docker → vsock → socket |
| Docker socket security (`0600`, no TCP 0.0.0.0) | MUST | vsock preferred |
| Code signing / entitlements | MUST | `com.apple.security.virtualization` |
| Privileged operations (avoid) | MUST | No sudo for VM start |
| Installation / upgrade / rollback / config migration | SHOULD / POST-MVP | Manual okay for MVP |
| Logs / diagnostics | SHOULD | `~/.harpoon/log`, `harpoon doctor` |
| DNS / IPv6 / VPN / proxy / CA / private registry / cred helpers | SHOULD / POST-MVP | IPv4+MUST, rest defer |
| Bind-mount ownership / UID/GID / case sensitivity / file watching | MUST correctness | Spike 4 |
| Sleep/wake / host network change / host reboot | MUST | |
| VM clock sync | MUST | |
| Docker API version compat / BuildKit / Compose / LazyDocker / Testcontainers | MUST/SHOULD | See compatibility.md |
| ARM64/x86 image behavior | SHOULD document | x86 not required |

## MVP Acceptance Tests

Harpoon v0.1 is **not complete merely because a Linux VM boots**. The following
workflows must succeed end-to-end:

1. `harpoon start`
2. `docker run --rm hello-world`
3. `docker pull postgres`
4. `docker volume create pgdata`
5. `docker network create devnet`
6. Run a persistent service using all three of: a named volume, a Docker network, and a
   published host port.
7. `docker build -t harpoon-test .`
8. `docker compose up -d`
9. `docker exec`, `docker logs`, `docker inspect`
10. `lazydocker` — capable of normal Docker API operations without Harpoon-specific
    support.
11. `harpoon stop` then `harpoon start` — persistent Docker state must remain intact.

## Memory Acceptance Test

Memory reclamation is an explicit product requirement, not a future optimization. The
project must eventually support a reproducible test approximately equivalent to:

1. Start Harpoon with no containers.
2. Measure host-side Harpoon VM resident memory.
3. Start a workload that materially increases guest memory consumption.
4. Confirm host VM memory increases appropriately.
5. Stop the workload or release its allocations.
6. Allow the guest to reach a reclaimable state.
7. Trigger or wait for Harpoon's memory policy.
8. Confirm that host resident memory materially decreases.

Harpoon optimizes for:

> Low post-workload resident memory, not merely low boot-time memory.

Exact numerical targets are established through baseline comparison against competing
runtimes rather than invented before measurement.

## Explicit Non-Goals for v0.1

The following are not MVP requirements:

- Kubernetes
- Multiple simultaneously managed Linux VMs
- Arbitrary Linux distributions
- GUI terminal emulator
- Extension marketplace
- Cloud orchestration
- Swarm
- Custom container runtime
- Custom image format
- Dockerfile replacement
- Registry marketplace
- Automatic HTTPS
- Complex DNS integration
- Advanced VPN integration
- Windows containers
- USB passthrough
- Production cluster management

## Kubernetes Support Policy

Harpoon v0.1 does not support Kubernetes. The GUI may eventually contain an Easter egg
under `Help └─ Kubernetes` that displays a dismissive message about Kubernetes and
performs no Kubernetes operation whatsoever. This behavior must never accidentally
become a dependency on Kubernetes.

