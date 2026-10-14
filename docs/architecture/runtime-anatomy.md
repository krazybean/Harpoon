# Canonical Runtime Anatomy

**Status:** Stage 1 observation of the implementation at `642d91d` (2026-10-13). This document describes the current checkout; it is not a refactor plan.

## End-to-end path

```text
macOS Docker client / Harpoon CLI / Tauri UI
        │
        ├─ Docker API over Unix socket `/tmp/harpoon-docker.sock` (0600)
        │
        └─ Harpoon CLI/control commands over local process and Unix sockets
                │
        foreground Harpoon Swift runtime (`harpoon/Sources`)
                │ owns VM lifecycle, state, bridges, disk, ports, diagnostics
                ▼
        Apple Virtualization.framework
        VZVirtualMachine + VZLinuxBootLoader + VZNAT + VirtioFS + VZVirtioSocketDevice
                ▼
        Alpine 3.22 ARM64 Linux guest
        kernel + initramfs `/init` + persistent ext4 root
                │
                ├─ `dockerd --host=unix:///var/run/docker.sock`
                │       │ Docker Engine owns Docker API and Docker objects
                │       ▼
                │   `containerd` (Docker-selected runtime service)
                │       ▼
                │   containerd shim(s) (one or more workload supervision processes)
                │       ▼
                │   `runc` / OCI runtime
                │       ▼
                │   Linux kernel namespaces, cgroups, mounts, networking
                │       ▼
                │   container process / workload
                │
                ├─ `socat VSOCK-LISTEN:2375` → Docker Unix socket
                └─ `socat VSOCK-LISTEN:2377` → `harpoon-mgmt-wrapper` → Python `harpoon-mgmt`
```

The Docker data path preserves Docker API ownership and does not implement a second container engine. The current host bridge does inspect selected HTTP requests for bind-path translation, port-reconciliation triggers, and Docker hijack detection before switching to raw streaming; that is boundary translation, not container semantics.

## Ownership and lifetime matrix

| Layer / process | Owner | Lifetime | Protocol / socket | Source evidence | Primary failure modes | Security / resource scope |
|---|---|---|---|---|---|---|
| Docker CLI, Compose, IDE clients | Client / user | Per command or client | Docker API over host Unix socket | `README.md`, `harpoon/Sources/HarpoonDockerCLI.swift` | Missing socket, daemon unavailable, API incompatibility | Host user access to a mode-0600 socket; Docker semantics remain downstream |
| Harpoon lifecycle process | Harpoon | Foreground child started by `harpoon start`; PID/lock guarded | Local process, `/tmp/harpoon.lock`, runtime metadata | `HarpoonLifecycleCLI.swift`, `HarpoonRuntimeState.swift`, `Lifecycle.swift` | Invalid config, stale PID/lock, VM start failure, cleanup failure | Host authority over VM and boundary resources; no container-state authority |
| Host Docker bridge | Harpoon `BridgeSet` / `UnixBridge` | VM running; per-client forwarding connections | Unix `/tmp/harpoon-docker.sock` 0600 → VZ virtio-vsock port 2375 | `Bridges.swift`, `UnixBridge.swift`, `RuntimeConfig.swift` | Bind/listen failure, vsock unavailable, partial I/O, guest socket unavailable | Local-only host endpoint; selected HTTP inspection plus raw streaming/hijack handling; port 2375 is guest-local via vsock |
| Host management bridge | Harpoon `BridgeSet` | VM running; per-client forwarding connections | Unix `/tmp/harpoon-mgmt.sock` 0600 → vsock port 2377 | `Bridges.swift`, `HarpoonMgmtCLI.swift` | Listener failure, guest management service unavailable, leaked socket | Privileged management boundary; local mode-0600 endpoint; no TCP/SSH exposure |
| VM | Virtualization.framework configured by Harpoon | Start until stop/failure | VZ object graph; serial/console markers | `main.swift`, `RuntimeConfig.swift`, `VMManager.swift` | Unsupported host, invalid configuration, VZ start failure, guest crash | Hypervisor boundary; configured CPU, memory, disk, network, VirtioFS, vsock resources |
| Guest kernel / initramfs `/init` | Guest Linux boot path; packaged by Harpoon builder | Boot until `switch_root` / fatal boot loop | `/dev/hvc0` markers, block device `/dev/vda`, module and filesystem syscalls | `tools/guest-builder/src/init` | Missing block device/modules, ext4 or resize failure, network/module failure, missing runtime closure | Root-level guest authority for boot, mounts, modules, cgroups, namespaces; failure loops intentionally preserve diagnostics |
| Guest persistent root | Docker/guest filesystem; disk lifecycle controlled by Harpoon | Persistent across Harpoon stop/start | Raw ext4 image attached as VM disk | `RuntimeConfig.swift`, `HarpoonLifecycleCLI.swift`, builder docs | Missing/corrupt image, insufficient filesystem geometry, stale runtime files | Harpoon owns VM disk provisioning/growth; Docker owns data stored inside `/var/lib/docker` |
| `dockerd` | Docker Engine | Started by guest init; readiness-gated before bridge success | Guest Unix `/var/run/docker.sock`; host reaches it only through vsock 2375 | `tools/guest-builder/src/init` around Docker start/bridge; `README.md` | Startup failure, missing runtime closure, unhealthy API, network/storage failure | Docker owns containers/images/networks/volumes/API/Compose/BuildKit semantics |
| `containerd` | Docker Engine distribution / guest runtime | Started/managed as part of Docker Engine stack | containerd internal Unix sockets and state under guest runtime paths | Guest init package closure and Docker runtime contract; exact socket path is runtime-managed | Missing binary, stale socket/state, service failure | Docker/containerd owns OCI lifecycle implementation; Harpoon observes through Docker/boundary health |
| containerd shim(s) | containerd | Workload-scoped; survives client command lifetime as needed | Internal containerd/shim protocol and inherited FDs | Runtime architecture and Docker/containerd contract; exact shim variant is environment-dependent | Shim exit, orphaned workload, I/O/exec failure | Guest process/cgroup scope for a container; not a Harpoon-owned container abstraction |
| `runc` / OCI runtime | containerd | Invocation-scoped for create/start/exec and lifecycle-managed workload state | OCI bundle, namespaces, cgroups, kernel syscalls | `tools/guest-builder/src/init` package list and `docs/requirements.md` | Missing runtime, invalid OCI config, namespace/cgroup/mount failure | Kernel-enforced isolation; Docker/containerd remain authoritative above it |
| Workload / container process | Docker Engine via containerd/runc | User-controlled container lifecycle | Linux namespaces, cgroups, pipes/PTYs, container networks/volumes | Docker contract in `docs/requirements.md`; management exec in `harpoon-mgmt` | Application exit, resource pressure, signal/TTY failure | Guest kernel isolation and Docker policy; Harpoon owns only host boundary translation |
| Guest management service | Harpoon guest management path, not Docker | Per vsock connection; child exec/shell per request | vsock 2377 → `socat` → JSON line or PTY stream | `harpoon-mgmt`, `harpoon-mgmt-wrapper`, init lines 900–923 | Invalid JSON/argv, missing interpreter, exec failure, PTY/session loss | Privileged guest command path; argv preserved without shell for `exec`; local host endpoint only |

## Resource scope

The containerd shim and `runc` process-lifetime descriptions below are
inferred from the Docker/containerd runtime contract; this Stage 0–1 work does
not claim direct live observation of every short-lived runtime process.

- Host scope: Harpoon process, UI process, Virtualization.framework service, host Unix sockets, disk image file, published-port forwarders, and VirtioFS shares.
- VM scope: configured vCPU and memory, guest kernel/cache, attached disk, VZNAT network, VirtioFS, vsock device, and guest processes.
- Docker scope: Docker Engine state, `/var/lib/docker`, containerd state, shim processes, OCI bundles, cgroups, container networks, volumes, images, and BuildKit.
- Workload scope: container cgroups, namespaces, process trees, filesystem mounts, and application resources.

These scopes must not be added together as if RSS, PSS, cgroup memory, VM footprint, and host `phys_footprint` were the same metric. The existing `docs/memory-model.md` remains the measurement reference.

## Failure propagation

1. Host validation/configuration failure prevents VM start and should leave no owned socket or lock behind.
2. Virtualization.framework failure transitions the Harpoon lifecycle to `FAILED`; `HOST_VZ_START_FAILURE` is a known qualified host condition in historical evidence.
3. Guest boot failure emits serial markers and remains in a diagnostic loop for boot-critical failures; host readiness must not be reported.
4. Docker failure prevents the Docker vsock bridge from becoming a usable host Docker socket.
5. A bridge failure preserves the distinction between “VM is running” and “Docker API is reachable”; the host must clean up only sockets it owns.
6. Container, shim, or workload failure is Docker/containerd/runc territory. Harpoon may expose boundary health and diagnostics but must not reimplement recovery semantics.

## Security boundaries

- macOS host to guest is a deliberate Virtualization.framework/vsock boundary, not a shared Docker socket filesystem mount.
- Host Docker and management sockets are Unix sockets with mode 0600; no wildcard TCP Docker API is part of the current path.
- Guest management uses vsock 2377 and `socat`; it has privileged exec/PTY capability and is therefore a higher-risk boundary than the Docker byte proxy.
- VirtioFS is a host-directory sharing boundary. Harpoon translates the host path and configures the share; Docker still owns the container bind-mount semantics after the path enters the guest.
- The guest kernel enforces process isolation. Docker/containerd/runc define workload lifecycle and OCI behavior within that boundary.
- Code signing and `com.apple.security.virtualization` are host packaging requirements, not substitutes for socket authorization or guest isolation.

## Source-of-truth and stale-document findings

### Current canonical observations

- Current source uses `/tmp/harpoon-docker.sock` and `/tmp/harpoon-mgmt.sock`, not `~/.harpoon/docker.sock`.
- Current production host runtime is Swift in `harpoon/Sources`, with a foreground VM-owning process launched by the lifecycle CLI. There is no current Rust `harpoond` implementation in this checkout.
- Current guest management is `socat` plus Python `harpoon-mgmt`; the proposed compiled `harpoon-guest` from the prior planning conversation does not exist yet.
- Current guest init still performs substantial boot/runtime reconciliation in shell. This phase records that fact and does not move it.

### Stale or qualified documents to revisit in a later stage

| Document | Finding |
|---|---|
| `docs/architecture.md` | Describes a future `harpoond` daemon, `~/.harpoon/docker.sock`, and a guest agent as if current; those claims conflict with current Swift paths and foreground lifecycle code. |
| `docs/requirements.md` | Socket and `harpoond` requirements use the same future `~/.harpoon`/daemon vocabulary; preserve as product requirements, but label implementation status before using as anatomy evidence. |
| `docs/phase2-acceptance.md` | Historical snapshot at commit `22415c1`, macOS 26.5.2, Docker 29.3.1, and a conditional pass; it is not a baseline for HEAD `642d91d`. |
| `docs/results/M17.md`, `docs/results/M18.md`, `docs/roadmap.md` | Historical acceptance and roadmap material contains qualified `BLOCKED`, `CONDITIONAL PASS`, and claims later corrected by subsequent evidence. `docs/roadmap.md` also still names commit `22415c1` and an older dirty-tree snapshot. It must not be summarized as an unconditional current green state. |
| `tools/guest-builder/README.md` | Says initramfs “apk-adds Docker at boot,” while the current init has offline closure checks and can skip package installation when artifacts are present. The document needs a later accuracy pass. |
| `harpoon/README.md` | Historical M1/M2 notes say a persistent background service is not implemented, which is consistent with current foreground ownership but conflicts with future architecture wording elsewhere. |
| `docs/compatibility.md`, `docs/building.md`, `harpoon/MIN_MACOS` | Minimum macOS claims differ: the build target is 15.1, compatibility says 13+, and the building guide says no minimum is declared. |
| `ui/harpoon-desktop/package.json`, `harpoon/Sources/HarpoonCLI.swift`, `harpoon/package.sh`, `harpoon/m11-test.sh`, `harpoon/m18-test.sh` | Version metadata is split between 0.1.1 and 0.1.0-era runtime/package/test names. This is a release/documentation consistency issue, not a Stage 0–1 runtime change. |

## Stage 1 boundary decision

No behavior refactor is justified by this anatomy alone. The next stage may investigate shell-to-compiled boundaries, but only after resource attribution and boot/host audits establish a concrete correctness, security, testability, or resource problem. Docker Engine/containerd/runc remain outside that migration boundary.

## Stage 4 host-runtime evidence

The Stage 4 audit maps the foreground Swift process as the owner of the VZ object graph, lifecycle transition writer, bridge listeners, timers, and ephemeral boundary paths. `Lifecycle` is authoritative for in-process transitions; `statusSnapshot()` is an external reconstruction from PID, flock, socket, connect, and log signals. This distinction is intentional evidence, not a claim that those probes are one state machine. See `docs/underbelly/stage-4-host-runtime-audit.md` and its machine-readable ownership map.

Live cleanup evidence from the earlier gates confirms mode-0600 Docker and management endpoints and normal stop/restart cleanup. The host audit found no dedicated unexpected-VM-exit observer, transactional bridge-start rollback, or central registry for closure-local accepted client FDs. These remain review findings; no behavior change is made here.

## Stage 2 live resource-attribution evidence

The repeatable collector in tools/stage-2-resource-gate.sh passed on
2026-10-13/14 with durable evidence in
docs/underbelly/stage-2-resource-attribution.json. It keeps host process
measurements, guest process/kernel measurements, and workload cgroup files as
separate domains and intentionally emits no grand total.

Observed on the permitting host:

- Harpoon runtime PID was attributable through status --json.
- A single Virtualization.framework service candidate was associated with the
  runtime disk through lsof: PID 1815,
  com.apple.Virtualization.VirtualMachine.
- Guest dockerd was observed with PPID 1 and containerd with PPID dockerd;
  containerd shims were observed with PPID 1 and one shim per observed
  workload container in the multi-container state.
- runc was directly observed during the lifecycle sampling window. Its
  invocation is transient; absence from an individual snapshot is not evidence
  that it was not used.
- The same collector captured State A (zero running containers), State B (one
  idle container), and State C (two active deterministic Alpine workloads).
  Docker inspect supplied identity and guest cgroup files supplied usage truth.
- Explicit exec and stop probes were sampled before and after the operation.
  Short-lived process changes remain bounded by the sampling cadence; the
  report records the observed window rather than claiming a complete event
  history.

Host vmmap physical-footprint output and any missing guest per-process PSS are
recorded as unavailable when the host or guest does not provide them. These
limitations do not authorize substituting RSS, Docker stats, or MemAvailable
for a different metric domain.

## Stage 3 live boot evidence

The Stage 3 phase map in `docs/underbelly/stage-3-guest-boot-phases.json` was
correlated with a fresh `/tmp/harpoon-serial.log` capture. The observed order
was initramfs block mount, runtime/module refresh, `HARPOON_PIVOT`, post-root
validation, disk geometry, network modules and forwarding, VirtioFS/balloon,
DHCP/DNS/HTTPS, offline APK skip, `/run` tmpfs, dockerd, vsock 2375,
`HARPOON_DOCKER_READY`, network readiness, and management vsock 2377 with
`HARPOON_MGMT_READY`. Host logs observed Docker readiness before management
readiness. Guest serial has no timestamps, so this is ordering evidence, not a
per-phase duration claim.

The same capture confirmed that a preserved 32 GiB block device with matching
ext4 geometry takes `HARPOON_DISK_NO_RESIZE`, and that the existing Docker
Engine/containerd/runc stack remains the guest runtime. Stage 3 documents the
shell state machine and future coordinator boundary; it does not change it.

## Stage 6 Docker boundary invariant

Stage 6 verified the ownership split against current source and an owned live
Docker lifecycle probe. Docker Engine remains authoritative for containers,
images, networks, volumes, API, published-port intent, Compose, BuildKit,
logs, exec, and container health. containerd/runc and the Linux kernel remain
authoritative below Docker. Harpoon may transport, observe, translate, and
invoke Docker, but must not infer or persist a second authoritative model of
Docker-owned resources. Harpoon-owned state is reconstructible boundary state:
VM lifecycle, sockets, translated host paths, host loopback forwarders, the
disk/resource envelope, guest management, and diagnostics.

The Stage 6 audit flags, without changing, duplicated context setup, foreign
context fallback behavior, stale guest-IP/duplicate-port reconciliation,
split readiness probes, and UI error-to-zero/config-path projections for later
cleanup. See `docs/underbelly/stage-6-docker-boundary-audit.md` and
`stage-6-docker-boundary-matrix.json`.

## Stage 7 diagnostics interface

`harpoon diagnostics snapshot --json` collects a point-in-time document with
separate `HOST`, `GUEST`, and `WORKLOADS` domains plus structured VM, Docker,
and management readiness. `diagnostics subscribe` is bounded polling over the
same collector; it does not create a daemon or claim event semantics the
current lifecycle cannot source truthfully. `diagnostics explain` documents
provenance and the deliberate omission of a grand total.

Docker inspect is used only for workload identity/metadata. Linux cgroup v2
files are used for workload resource truth. Missing `/proc`, cgroup, host
process, or Virtualization.framework observations remain explicitly
`unavailable`. The contract is documented in
`docs/underbelly/stage-7-diagnostics-contract.md`.
