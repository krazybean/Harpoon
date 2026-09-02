# Architecture

This document describes Harpoon's system architecture: its components, the boundaries
between them, and the data flows that connect them. For scope and acceptance criteria,
see [Requirements](requirements.md). For memory behavior, see the
[Memory Model](memory-model.md).

## Design Principles

1. **Reuse mature standards.** Prefer existing OCI standards, Docker Engine, the Docker
   API, containerd, BuildKit, Linux kernel facilities, and Apple virtualization
   facilities. Harpoon's value exists primarily at the macOS/Linux boundary.
2. **Measure before optimizing.** Establish benchmarks before reproducing optimization
   techniques other products claim to use.
3. **Maintain architectural boundaries.** No layer may silently duplicate another
   layer's authority (see [Layer Ownership](#layer-ownership)).

## System Overview

Harpoon runs a minimal Linux appliance guest on macOS via Apple's
`Virtualization.framework`. All Docker traffic crosses a deliberate host-to-guest
transport rather than a shared filesystem socket mount.

```text
macOS
│
├── docker
├── docker compose
├── lazydocker
├── IDE integrations
├── Testcontainers
│
│   Docker API
│
▼
~/.harpoon/docker.sock
│
▼
Harpoon socket bridge
│
▼
Virtio/vsock transport
│
▼
Minimal Linux guest
│
├── Docker Engine
├── containerd
├── BuildKit
├── guest agent
└── containers
```

Separately, control-plane clients talk to the Harpoon daemon:

```text
Harpoon CLI ───────┐
                   │
Tauri UI ──────────┼── Harpoon control API ── harpoond
                   │
Diagnostics ───────┘
```

`harpoond` owns the virtual machine and its lifecycle.

## Layer Ownership

Each layer owns exactly one authority. No layer silently duplicates another's.

| Layer                        | Owns                                  |
| ---------------------------- | ------------------------------------- |
| Harpoon daemon (`harpoond`)  | Harpoon state, VM lifecycle           |
| Docker Engine                | Container/image/volume/network state  |
| Linux kernel                 | Container isolation                   |
| Virtualization.framework     | Hardware virtualization               |
| Tauri UI                     | Presentation only                     |

## Components

### harpoond (daemon)

The single authority over Harpoon runtime state and the VM. Responsibilities:

```text
harpoond
├── VM lifecycle            (boot, pause/shutdown, crash recovery)
├── disk lifecycle          (create, grow, attach, persist)
├── guest communication     (control channel to the guest agent)
├── Docker socket bridge    (Unix socket → vsock proxy)
├── memory controller       (policy engine driving ballooning/reclaim)
├── network integration     (VM↔macOS boundary, published ports)
├── runtime state           (persisted daemon state under ~/.harpoon)
└── diagnostics             (health, logs, doctor checks)
```

The daemon operates independently of all clients. Closing any client — including the
GUI — must not stop `harpoond`, the VM, or running containers. Frontend concerns must
not be embedded into the daemon.

### Socket bridge

Exposes a Docker-compatible Unix socket on the host at `~/.harpoon/docker.sock` and
proxies raw Docker API traffic into the guest over virtio sockets (vsock). Requirements:

- Transparent byte-stream proxying — no interpretation or filtering of Docker API
  payloads beyond what connection management requires.
- Must not depend on exposing `/var/run/docker.sock` through a shared filesystem.
- Must tolerate concurrent connections from multiple client processes.

### Management channel (Stage 3A, vsock 2377, no SSH, no TCP)

Dedicated host↔guest control channel over virtio-vsock, separate from Docker port 2375.

- Guest listener: `socat VSOCK-LISTEN:2377,fork EXEC:/usr/local/bin/harpoon-mgmt` (vsock-only, no TCP bind, no SSH daemon)
- Host bridge: `BridgeSet` listens on unix `0600` at `/tmp/harpoon-mgmt.sock` → vsock `2377` per-connection proxy (no wildcard TCP, no LAN exposure)
- Protocol: JSON-line request `{"op":"exec","argv":[...]}` or `{"op":"shell"}` over single vsock connection; exec preserves argv boundaries (no `/bin/sh -c`), returns `{"exit":N,"stdout":"...","stderr":"..."}`; shell spawns `/bin/sh` with PTY via `pty.openpty()` and proxies raw bytes bidirectionally
- CLI: `harpoon exec -- <argv...>` (host returns guest exit status, prints stdout/stderr, failure messages: Harpoon VM is not running, Guest management service is not ready, Connection to guest management service failed, Guest command exited with status N); `harpoon shell` opens interactive PTY (stdin/stdout/stderr, raw mode, clean EOF)
- Readiness: guest emits `HARPOON_MGMT_READY` to serial (`/dev/hvc0`) after mgmt listener is listening; host `VMManager` observes it separately from `HARPOON_DOCKER_READY` (Docker readiness does NOT depend on mgmt); `harpoon doctor` and `harpoon status` report `Management: ready` and check `0600` socket + `HARPOON_MGMT_READY`
- Security/trust: vsock-only reachability (no TCP networking), no SSH, no host wildcard ports; logged-in macOS user owning the VM is trusted for root-level guest management (no auth theater); host unix sockets preserve `0600` permissions

### Guest agent

A small process inside the Linux guest, started early in boot. Responsibilities:

- Health/status reporting to `harpoond` (Docker daemon state, guest uptime).
- Guest memory telemetry for the memory policy engine (see [Memory Model](memory-model.md)).
- Cooperative reclaim actions (e.g., dropping reclaimable page cache) when instructed.

### Memory controller

Part of `harpoond`. Observes host and guest memory signals and drives Apple-supported
memory ballooning primitives. Policy details are specified in the
[Memory Model](memory-model.md); the architectural constraint is that reclamation must
be observable and measurable, and the controller must avoid pathological oscillation.

### GUI (future)

A Tauri + React + TypeScript desktop application. It is strictly a **client of the
control API** and must never own the VM lifecycle. Primary value is operational
visibility (runtime, memory, storage, networks), not a second container state model.


## CLI & Background Lifecycle (M7)

Harpoon M7 adds background lifecycle without turning the VM into a daemon framework.

- `harpoon run` = foreground/debug (Phase 1 behavior, logs to terminal, Ctrl-C)
- `harpoon start` = background (spawns `harpoon run` via Process, logs to `~/Library/Application Support/Harpoon/harpoon.log`, waits ≤60s for `HARPOON_RUNNING`)
- `harpoon stop/status/logs/restart/version` manage the runtime process
- Single instance via `/tmp/harpoon.lock` (flock), socket ownership via `ownsDockerSocket` and `/tmp/harpoon.lock`, status from live lock/socket/proc, not just pid file
- PID safety via `proc_pidpath`, stale recovery, no blind kill
- Log rotation `harpoon.log` → `harpoon.log.1` on start

See [Lifecycle](lifecycle.md) for process model, file layout, and failure semantics.


## Docker Native Integration (M8 + Stage 3B)

Harpoon integrates via standard Docker contexts, no Desktop dependency. Stage 3B adds Finder-safe discovery and host-integration hardening.

- **Canonical discovery**: `PATH` → `/opt/homebrew/bin/docker` → `/usr/local/bin/docker` → `/usr/bin/docker` (no `zsh -l -c`, no shell sourcing). `harpoon doctor` reports `Docker CLI ................. PASS /opt/homebrew/bin/docker` or `FAIL — Docker CLI not installed/found`. All Tauri `docker` invocations use the same resolved absolute path.
- **Harpoon context**: `harpoon` → `unix:///tmp/harpoon-docker.sock` via `harpoon docker setup` (idempotent create/repair via `context update` → `rm -f` → `create`, exact endpoint `unix:///tmp/harpoon-docker.sock`, never switches default `docker context` — Harpoon uses `docker --context harpoon ...`). Desktop app ensures context on first relevant use via `ensure_harpoon_context` (safe to run repeatedly, only `harpoon` mutated).
- **Compose v2**: Detected separately via `docker compose version` (same resolved binary); `harpoon doctor` reports `Docker Compose plugin ...... PASS v5.1.0` vs `FAIL` with hint. Non-compose `docker` operations continue when Compose missing.
- **Credential helper**: Non-destructive inspect of `~/.docker/config.json` (`DOCKER_CONFIG` respected); if `"credsStore":"desktop"` and `docker-credential-desktop` not in `PATH`/standard locations, `harpoon doctor` warns `Docker credential helper ... WARN — config references docker-credential-desktop but helper not installed` (Harpoon never deletes `credsStore`).
- Context survives stop/start; buildx works via standard context

See [Docker Integration](docker-integration.md) and [Lifecycle](lifecycle.md).

## Persistent Storage (Stage 3C)

Harpoon separates **immutable template** `assets/guest/harpoon-root.img` (pristine, `containers=0 images=0 volumes=0`, sparse `2 GiB` logical, `~962M` physical, small for distribution) from **mutable user disk** `~/Library/Application Support/Harpoon/data/harpoon-root.img` (fallback `/tmp/harpoon-runtime/data/harpoon-root.img`, persistent, grow-only, sparse, never silently replaced).

- **Provenance**: template → `harpoon-root.img` built via `tools/guest-builder` (ext4 directly on `/dev/vda` raw block, no partition, `Docker data-root=/var/lib/docker` wholly on that device).
- **First provision**: copied **once** via `cp -c` (APFS clone) + `ditto` fallback only when mutable disk does **not exist**. Once provisioned, never size/hash-compared, never overwritten on start/update, never shrunk. Corrupt → diagnostic FAIL, explicit destructive reset only (not implemented implicit).
- **Capacity model**: distribution template stays small; **default first-provision 32 GiB logical sparse** (sparse, `32*1024^3=34359738368`, physical stays ~1G until used; logical = `truncate -s` on sparse image only — ext4 filesystem size requires guest `resize2fs` and remains unverified until VM boots) because workloads exhausted 2 GiB (`no space left on device`). Existing disks `<32G` (e.g. 8G/16G) remain valid — no auto-enlarge on boot, no rejection. Supported `G/GiB/M/MiB` (e.g. `12G 16G 24G 32G`), integer only, rejects zero/negative/malformed/overflow/` <2G`/shrink with clear messages.
- **First-provision interface**: `harpoon start --disk-size 16G` (when no disk; if disk exists, tells `use harpoon disk resize 16G`), plus persistent `harpoon config set/get disk-size 16G` (`~/Library/.../config.json` `diskSize`); desktop consumes same config; `HARPOON_DISK_SIZE` env also considered. No `zsh -l` needed.
- **Disk commands**: `harpoon disk status` (backing path, logical/physical sparse, FS capacity/used/free via `df -B1` over vsock `2377` when running else `unavailable`, inode, template immutable, configured), `harpoon disk resize 16G` (grow-only, `requested <= current` rejected, never shrink).
- **Safe resize architecture**: **OFFLINE grow** preferred (ext4 journaled). `harpoon disk resize` requires `VM stopped`, validates regular file, grows sparse backing via `truncate`/`ftruncate` (old FS remains valid), verifies `logical == requested` and `inode` unchanged. **No macOS `e2fsprogs` required** — production app owns pathway via guest `e2fsprogs` (`apk add e2fsprogs`) and auto `resize2fs /dev/vda` (safe **online** `resize2fs` on mounted ext4, journaled atomic) on next boot if `blockdev --getsize64 > df -B1`. If ext4 inside partition would use `parted`+`resize2fs`, but Harpoon is raw so direct `resize2fs`.
- **Failure atomicity**: backing grown without FS grown is valid old FS; retry detects `backing > filesystem` and re-runs `resize2fs` (doctor warns `backing > filesystem — pending resize`). Never copies template over user disk as recovery.
- **Doctor/status**: `Persistent disk ............ PASS`, `Logical capacity`, `Host allocation sparse`, `Filesystem capacity/free`, warnings for `backing>FS` (interrupted), host low free `<2G`, `FS low <512M`, ` <8G` with `harpoon disk status` details. When stopped, backing facts authoritative, no fake FS.
- **Desktop integration**: Tauri `get_disk_status`/`resize_disk` (via `harpoon disk` CLI single source), `harpoon doctor` storage included. Provision choices `8 16 32 Custom` via `harpoon config set disk-size`/`harpoon start --disk-size`/`harpoon disk resize` without major UI redesign.
- **Sanitation**: template stays `0/0/0` (`tools/guest-builder/verify-root.sh`); tests use copies, never enlarge template.

See [Installation](installation.md) (disk prerequisites) and `harpoon disk --help`.


## Developer Ergonomics (M10)

- Persistent config `~/Library/Application Support/Harpoon/config.json` (`cpus`, `memory`), precedence `CLI > config > env > defaults`, atomic writes, validation `1...8` and `512|768|1024`
- `harpoon help` (product description, examples, diagnostics), `harpoon doctor` (PASS/WARN/FAIL for host/Harpoon/Docker), `harpoon status --json`, `harpoon logs --path`, `harpoon docker env`
- `harpoon version` → `Harpoon 0.1.0-dev`, stable exit codes (`0`, `1`, `2`, `5`, `7`, `10`)
- `harpoon start` default `cpus 2 memory 1024`, `stop` idempotent, `restart` preserves config unless CLI override
- Human-friendly, no color dependency, no shell mutation, socket `0600` preserved

See [Configuration](configuration.md), [Troubleshooting](troubleshooting.md).

## Native macOS Virtualization Architecture

Harpoon is built directly on Apple's native `Virtualization.framework`.

Rather than bundling a separate third-party hypervisor, Harpoon uses Apple's native `Virtualization.framework` for Linux virtualization, including Virtio networking, block storage, VirtioFS, vsock, and memory ballooning.

Proven primitives already used:

- `VZVirtualMachine`, `VZGenericPlatformConfiguration`
- `VZLinuxBootLoader` (direct kernel+initramfs boot, `Image-virt` `6.12.94-0-virt`)
- `VZVirtioBlockDeviceConfiguration` + `VZDiskImageStorageDeviceAttachment` (`ext4` persistent root)
- `VZNATNetworkDeviceAttachment` + `VZVirtioNetworkDeviceConfiguration` (VZNAT, `virtio_net`)
- `VZVirtioSocketDeviceConfiguration` + `VZVirtioSocketDevice` (vsock `AF_VSOCK` `2375` Docker, `2377` management)
- `VZVirtioFileSystemDeviceConfiguration` / `VZSingleDirectoryShare` / `VZSharedDirectory` / VirtioFS (`harpoon-share` → `/mnt/harpoon-share`)
- `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` / `VZVirtioTraditionalMemoryBalloonDevice` (`targetVirtualMachineMemorySize`)

Why direct `Virtualization.framework`:

- macOS owns the virtualization substrate — no separate hypervisor to ship, version, or secure
- Harpoon focuses on container-runtime integration, transport (`vsock` byte-stream), filesystem sharing (`VirtioFS`), networking (`VZNAT` + `127.0.0.1` forward), lifecycle, and UX
- No need to ship/manage a separate hypervisor implementation
- This is a design simplification and macOS-native integration point, not a blanket performance guarantee — Harpoon does not claim zero-overhead, bare-metal, or faster-because-native.

## Feasibility Spike Results (Spikes 1–5 Complete)

Spike 1 PASS — Linux VM boot (`VZLinuxBootLoader` `HARPOON_SPIKE_OK`/`SHUTDOWN_OK`)
Spike 2 PASS — Docker API bridge + container execution (`/tmp/harpoon-docker.sock` → vsock `:2375` → `dockerd`, `hello-world`)
Spike 3 PASS — Container networking + published ports (`docker0` bridge, `ip_forward 1`, `127.0.0.1:8080` forward)
Spike 4 PASS — VirtioFS host sharing / bind mounts (`/tmp/harpoon-share` → `/mnt/harpoon-share` → `-v /mnt/harpoon-share:/workspace`)
Spike 5 PASS — Memory characterization + lower observed footprint vs Docker Desktop (see docs/results/SPIKE5.md)

Feasibility phase complete. These do not imply production readiness.

### Phase 1 — Production Runtime Foundation + VM Lifecycle (M1+M2) — COMPLETE

Production runtime graduated from spikes: `harpoon/` Swift runtime owns `VZVirtualMachine`, `VZLinuxBootLoader`, `VZGenericPlatformConfiguration`, cpu/memory/block/NAT/VirtioFS/vsock/balloon, validation, start/stop.

- **M1 Runtime Foundation — COMPLETE:** `harpoon/Sources/RuntimeConfig.swift` (2 vCPU 1024 MiB default 768/512 diagnostic, kernel `spike1/cache/Image-virt`, initramfs `spike2/cache/harpoon-docker-initramfs.cpio.gz`, disk `spike2/cache/harpoon-root.img`, share `/tmp/harpoon-share` tag `harpoon-share`, socks `/tmp/harpoon-docker.sock` 0600 + `/tmp/harpoon-control` 0600, forward `127.0.0.1:8080`, serial `/tmp/harpoon-serial.log`), `harpoon/Sources/VMManager.swift` (build/validate/start), `harpoon/Sources/Bridges.swift` (unix bridge full-duplex half-close, balloon control per-client buffered, host forward loopback-only), `harpoon/Sources/Lifecycle.swift` centralized `HARPOON_STATE`, `harpoon/build.sh` `swiftc -framework Virtualization` + `codesign com.apple.security.virtualization` 327K, spike mechanisms preserved natively (no QEMU/Lima/HyperKit).

- **M2 VM Lifecycle — COMPLETE:** `STOPPED->STARTING->BOOTING->DOCKER_READY->RUNNING->STOPPING->STOPPED` + `FAILED reason=`, readiness distinction (`VZVirtualMachine.start()` alone ≠ RUNNING, `HARPOON_DOCKER_READY` gate), bridges start only after DOCKER_READY, graceful `STOPPING->STOPPED` cleans `/tmp/harpoon-docker.sock` `/tmp/harpoon-control` listeners FDs DispatchSources, preserves `harpoon-root.img` + `/run tmpfs` fix, SIGINT/SIGTERM -> controlled STOPPING, failure handling (invalid config, VM start failure `VZErrorDomain 1`, guest timeout, DOCKER_FAILED, socket failure -> FAILED with reason, no orphan), restartability `start->stop->start` without reboot/delete/manual cleanup, observability `HARPOON_STATE` + `HARPOON_MEMORY_CONFIG_*` `HARPOON_BALLOON_*` `HARPOON_VIRTIOFS_*` etc. Acceptance A-H verified via spike evidence + production failure path (`/nonexistent/kernel` -> `FAILED reason=kernel not found`, `VZErrorDomain 1` -> `BOOTING->FAILED` with `HOST_VZ_START_FAILURE` and ephemeral cleaned); full `RUNNING` lifecycle blocked at acceptance time by transient host `VZErrorDomain 1` (host-wide, `spike1/build/harpoon-spike1` also fails, REBOOT_SKIPPED) — requires host reboot to re-prove A-G, production correctly enters FAILED. See `harpoon/README.md` for component boundaries, build, and acceptance evidence.

Do not mark M3+ complete merely because some functionality exists in spikes.

### Phase 1 — M3 Docker Compatibility — PASS (harness corrected, live prove pending host recovery)

M3 inspected `harpoon/Sources/*`, bridge, lifecycle, socket 0600, Docker API, docs/roadmap. Compatibility surface documented in `harpoon/M3_COMPATIBILITY.md`, bounded test `harpoon/m3-test.sh` (error semantics now case-insensitive `no such|not found` with exit-code proof) and `harpoon/regression-bridges.sh` (bounded 5-client capture per `hi N`, exit-code + output, socket persistence) cover core lifecycle, image, build, exec/logs/stdin, env/workdir/entrypoint/user, labels/filters, network/volume objects, concurrency, large streaming, error passthrough. Host CLI `29.3.1 API 1.54` -> guest `28.3.3 API 1.51` auto-downgrade transparent (no HTTP rewriting, half-close keep-alive). Harness fixes: `m3-test.sh` now proves `inspect`/`rm` both exit 1 and `no such|not found` case-insensitive; `regression-bridges.sh` now launches exactly five `docker run --rm alpine:3.22 echo "hi N"` capturing per-N logs/status, verifying each `exit 0` and `hi N`, reporting actual output on failure, then verifying sockets `srw------- 0600`. No bridge code touched for `1/4` (as instructed). Live rerun requires `HARPOON_RUNNING` (currently host `VZErrorDomain 1` after `16:48` kill, pending 90s+ idle recovery, see `12:04` 344K fix). Before fix, concurrent 5x hid failures; after fix, `11:44 RUNNING` showed `5x permission denied` due to stale bridge (fixed 12:04 `HostPathTranslator` not yet running). Pending host recovery: `harpoon/build/harpoon > /tmp/harpoon.log 2>&1 &` then `DOCKER_HOST=unix:///tmp/harpoon-docker.sock bash harpoon/regression-bridges.sh` and `bash harpoon/m3-test.sh` must both PASS before marking M3 PASS.

### Phase 1 — M4 Filesystem & Storage — IN PROGRESS (code complete, live prove pending host VZ recovery)

M3 inspected `harpoon/Sources/*`, bridge, lifecycle, socket 0600, Docker API, docs/roadmap. Compatibility surface documented in `harpoon/M3_COMPATIBILITY.md`, bounded test `harpoon/m3-test.sh` covers core lifecycle, image, build, exec/logs/stdin, env/workdir/entrypoint/user, labels/filters, network/volume objects, concurrency (5 parallel `docker run`), large response streaming (~2 MB 50000 lines), error passthrough, cleanup, regression. Host CLI `29.3.1 API 1.54` -> guest daemon `28.3.3 API 1.51` auto-downgrade transparent (no HTTP rewriting, protocol-transparent full-duplex half-close keep-alive). Socket `/tmp/harpoon-docker.sock` 0600 verified, `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` contract.

No code change required: `Bridges.swift` already protocol-transparent concurrent keep-alive 8192 loops per spec sec 3/12/13. Daemon label `harpoon.runtime=true` deferred per sec 15 (requires guest initramfs rebuild, would be `dockerd --label harpoon.runtime=true` invasive). Live acceptance blocked at 2026-08-25 15:12-15:13 by host-wide transient `VZErrorDomain 1 Internal Virtualization error` — `harpoon/build/harpoon` and `spike1/build/harpoon-spike1` both `BOOTING->FAILED HOST_VZ_START_FAILURE` even after 60s pause, `harpoon/m3-test.sh` correctly `FAIL socket missing (harpoon not RUNNING)` and `docker version` `permission denied` when VM down. `M1/M2` host failure handling verified `FAILED reason=...` no orphan. Requires host reboot then: `bash harpoon/build.sh; harpoon/build/harpoon` -> `HARPOON_RUNNING` then `bash harpoon/m3-test.sh` (see `harpoon/README.md` post-reboot steps). Do not claim Compose/M4/M5.

### Phase 1 — M5 Networking & Port Publishing — CODE COMPLETE (live prove pending host VZ recovery)

Productionized dynamic Docker `-p` publishing, replacing spike hardcoded `127.0.0.1:8080->guest:8080`.

- **Packet path:** `macOS curl 127.0.0.1:<HostPort> -> Harpoon host TCP listener (127.0.0.1:HostPort, per-mapping DispatchSource) -> guest VZNAT IP:<HostPort> (via TCP connect to discovered guest IP) -> guest Docker iptables DNAT/docker0 -> container:<ContainerPort>`. Harpoon does NOT implement container NAT; Docker owns bridge/netns/iptables/DNS.
- **Hardcoded location removed:** `Bridges.swift: startHostPortForward(guestIP:)` binding `config.hostForwardPort=8080`/`guestForwardPort=8080` (`RuntimeConfig.hostForwardPort/guestForwardPort`) and `startHostForwardPoll()` 15s HARPOON_GUEST_IP poll with single `hostForwardFd/Source`. Replaced by `PortForwardManager.swift` + `Bridges.startPortForwarding()` + `guestIPPoll`.
- **Docker source of truth:** `GET /containers/json?all=1` via vsock `2375` (VZNAT-independent), parse `State=="running"` and `Ports[]` (`IP`, `PublicPort`, `PrivatePort`, `Type`). `NetworkSettings.Ports` equivalent but `Ports[]` already contains `PublicPort` actual (ephemeral case). Chosen over `HostConfig.PortBindings` pre-start request because `Ports` reflects daemon-assigned actual and running state; still triggered on `POST /containers/create|/start|/stop|/restart|DELETE` via HTTP parser `HARPOON_HTTP_REQUEST` to schedule sync 0.8s/2s.
- **Smallest seam:** narrow observation of `containers/*` API already parsed by M4 HTTP streaming parser; no general Docker API impl, no parallel config format. Timer poll `2s` + event-triggered `scheduleSync` reconciles `PortForwardManager.reconcile(desired:guestIP:)`.
- **Transparency:** `vsock->client` response path unchanged transparent half-close; `client->vsock` HTTP parser forwards non-`containers/create` verbatim; published-port forwarding is separate TCP listeners, not HTTP-interpreted.

Layers: `macOS host -> Harpoon listener -> VZNAT guest address -> guest Docker HostPort -> docker0/container`.

Dynamic design: `PublishedPortKey = 127.0.0.1:HostPort/tcp -> Forward{fd, DispatchSource, guestIP:HostPort, containerId/Name}`. `PortForwardManager` owns `N` forwards, `hostAddrForDockerIP` maps `0.0.0.0`/`""`/`::` -> `127.0.0.1` loopback-only safety (prevents LAN exposure; ordinary `-p 18080:80` remains `127.0.0.1` reachable). Explicit `127.0.0.1:18087:80` binds guest `127.0.0.1:18087` and is **deferred Phase 1 limitation** — not reachable via VZNAT `guestIP:18087` (`192.168.64.x`), characterized in `harpoon/m5-test.sh` Test 8 as `[m5] CHARACTERIZED explicit Docker 127.0.0.1 HostIp deferred: guest-loopback binding is not reachable via VZNAT guestIP forwarding`, not a failure of dynamic `-p`. `HARPOON_PORT_FORWARD_ADD/LISTENING/REMOVE/COLLISION/RESTORE` grep-friendly. TCP required, UDP deferred (`Type=="udp"` skipped, `HARPOON_PORT_FORWARD_SKIP`). IPv4 loopback required, IPv6 not claimed. Ephemeral `HostPort=0`/`""` discovered via `PublicPort` assigned, mirrored if present else deferred. Multiple ports per container and multiple containers concurrent via `forwards` dict keyed by hostAddr:hostPort/proto.

Lifecycle: `START` -> `sync` adds listener and forwards to `guestIP:HostPort`; `STOP` -> state != running removes; `REMOVE` -> remove; `RESTART` -> re-add; `Harpoon restart` -> after `HARPOON_GUEST_IP_DISCOVERED` + `HARPOON_DOCKER_READY`, `PortForwardManager` polls and `RESTORE`s active running mappings without recreation.

Security: VZNAT boundary, only published ports exposed, no blanket forward, no privileged magic, sockets `0600`.

Code: `harpoon/Sources/PortForwardManager.swift` (551K), `Bridges.swift` HTTP parser triggers sync, `RuntimeConfig.hostForwardPort` retained but unused (legacy stub `_legacy_startHostPortForward` no-op).

Live acceptance blocked by same host `VZErrorDomain 1` since 2026-08-25 10:01 (re-tested 18:10 3 VirtioFS devices `validate OK` -> `BOOTING->FAILED HOST_VZ_START_FAILURE`), pending host reboot. After reboot: `harpoon/build/harpoon > /tmp/harpoon.log &` then `bash harpoon/m5-test.sh` should PASS single/multi/repeated/concurrency/container-to-container/outbound/stop-start-remove/collision/no-publish/ephemeral/UDP/M1-M4.

## Spike vs Production Guest Root


Harpoon deliberately proves container execution with the smallest valid ramdisk.

- **Spike guest (Spike 2):** `initramfs-only` root (`rootfs`/`tmpfs` from `initramfs.cpio.gz` backed by RAM) is acceptable for proof. Docker/containerd state lives on `tmpfs`. Docker checks `DOCKER_RAMDISK=true` to avoid normal `pivot_root` semantics that fail with `invalid argument` when the daemon itself runs from a ramdisk (`runc create: pivot_root .: invalid argument`). This is a deliberate spike workaround, not a production default.

- **Production Harpoon guest:** prefer a tiny **block-backed writable Linux root filesystem** (e.g., small `ext4` `qcow2`/`raw` on `VZDiskImageStorageDeviceAttachment`) for Docker/containerd state (`/var/lib/docker`, `/var/lib/containerd`), normal `pivot_root` semantics, persistence across VM restarts, and standard overlay2 operation. The ramdisk approach does not provide durable storage and changes container isolation semantics.

Do not redesign the spike guest to block-backed storage in this task; the note records intent.

Production Spike 2+ guest is already block-backed: `spike2/cache/harpoon-root.img` `2G` sparse raw `ext4` `VZVirtioBlockDevice` with `pivot_root`; spike ramdisk workaround (`DOCKER_RAMDISK=true`) retired. Persistent ext4 lesson: `/run` must be `tmpfs` ephemeral (`mount -t tmpfs tmpfs /run` before `containerd`/`dockerd`, `/var/run → ../run` preserved), otherwise stale `PID`/`sock` survives reboot (`PERSISTED_RUNTIME_STATE` `containerd did not exit successfully`).

Do not redesign the spike guest to block-backed storage in this task; the note records intent.

## Linux Guest Philosophy

The guest is an **appliance**, not a general-purpose distribution. Avoid unnecessary
services; include only what is required to execute OCI/Docker workloads reliably.

Boot path:

```text
Linux kernel
    ↓
minimal init
    ↓
mount required filesystems
    ↓
configure networking
    ↓
start guest agent
    ↓
start Docker/containerd
```

Candidate guest components:

- Linux kernel (ARM64)
- Minimal userspace
- cgroups v2
- overlayfs
- virtiofs (bind mounts)
- virtio networking
- vsock (host-guest transport)
- virtio balloon (memory reclamation)
- nftables/iptables as required
- Docker Engine, containerd, BuildKit
- Harpoon guest agent

Explicitly excluded: a desktop environment, and services included merely because
conventional distributions normally provide them.

## Data Flows

### Data plane (Docker API)

```text
docker / compose / IDE
        │  Docker API over Unix socket
        ▼
~/.harpoon/docker.sock  (host)
        │  byte-stream proxy
        ▼
vsock  (host ⇄ guest)
        │
        ▼
Docker Engine in guest  ──►  containers
```

### Control plane

```text
CLI / GUI / diagnostics
        │  Harpoon control API
        ▼
      harpoond
        │  control channel (vsock)
        ▼
    guest agent
```

### Storage

- **Named volumes** live on Linux-native persistent VM storage
  (`/var/lib/docker/volumes/`). The VM disk persists across Harpoon restarts.
- **Bind mounts** route macOS directories into containers via VirtioFS:
  `macOS dir → VirtioFS → guest → Docker bind mount → container`.
  No custom filesystem caching in the first implementation; measure before optimizing.

### Networking

Docker Engine owns bridge networking, container-to-container traffic, user-defined
networks, aliases, and embedded DNS. Harpoon owns only the VM⇄macOS boundary:
outbound Internet access and `localhost:<port>` reachability for published ports.

```text
container → docker bridge → Linux VM → Harpoon host networking → macOS
```

## Repository Direction

Illustrative layout — to be validated before freezing crate boundaries:

```text
harpoon/
├── Cargo.toml              # workspace
├── crates/
│   ├── harpoon-core/       # shared types, config, state
│   ├── harpoon-daemon/     # harpoond
│   ├── harpoon-cli/        # harpoon
│   ├── harpoon-vz/         # Virtualization.framework integration
│   ├── harpoon-socket/     # Docker socket bridge
│   ├── harpoon-memory/     # memory policy engine
│   └── harpoon-protocol/   # host⇄guest wire protocol
├── guest/
│   ├── kernel/
│   ├── rootfs/
│   ├── init/
│   └── agent/
├── ui/harpoon-desktop/     # Tauri GUI (client only)
├── docs/                   # these documents + ADRs
├── scripts/
└── tests/
```

Notes on the stack:

- Rust is the primary language for the daemon, CLI, state management, host/guest
  communication, socket proxy, memory policy engine, diagnostics, and guest agent.
- The macOS virtualization integration uses `Virtualization.framework`, likely via
  Objective-C bindings or a small ObjC/Swift bridge inside `harpoon-vz`. The core must
  not move into Swift merely for convenience; a documented technical limitation would be
  required (recorded as an ADR in `docs/decisions/`).


## Architectural Claim Classification

Per hardening requirement, every major assumption is classified:

| # | Claim | Status | Evidence / Next Step |
|---|-------|--------|----------------------|
| A1 | `Virtualization.framework` can boot ARM64 Linux (PROVEN 2026-08-25 via `VZLinuxBootLoader` `HARPOON_SPIKE_OK`/`SHUTDOWN_OK`; earlier `Code=1` transient marked HISTORICAL) via `VZLinuxBootLoader` + `VZGenericPlatformConfiguration` | **KNOWN** | Apple docs + `VZLinuxBootLoader` header (macOS 11+). `isSupported==true` on host (Spike 1 pre-check). |
| A2 | Harpoon can control VM lifecycle (create/configure/start/stop) from Rust without Swift | **UNVERIFIED** | Requires ObjC bridging (`objc2`/`block2`). No existing Harpoon code demonstrates it. Spike 1 must prove Rust vs minimal Swift bridge. |
| A3 | Rust `objc2` + `block2` bindings are sufficient and cleaner than a small Swift bridge | **EXPERIMENTAL** | `objc2` 0.6.4 mature, but Virtualization.framework is callback/block-heavy; Swift bridge may be smaller. Spike 1 evaluates both. |
| A4 | `VZLinuxBootLoader` kernel+initramfs direct boot is appropriate (vs disk image) | **SUPPORTED** | Direct boot is documented for Linux guests; disk boot (`VZDiskImageStorageDeviceAttachment` + EFI) adds complexity. Spike 1 uses direct boot. Spike 2+ may evaluate disk for persistence. |
| A5 | Docker API transport `host docker.sock → vsock → guest docker.sock` is correct | **KNOWN** (Spike 2 PASS) | Proven `harpoon-spike2-vsock` `Unix 0600 → VZVirtioSocketDevice :2375 → socat VSOCK-LISTEN:2375 → /var/run/docker.sock` half-close `BRIDGE_*`, `docker version`/`hello-world` via `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` |
| A6 | `localhost:<port>` reachability for `docker -p` published ports | **KNOWN** (Spike 3 PASS) | Proven `127.0.0.1:8080` `Harpoon host TCP forward → guest VZNAT IP:8080 → Docker DNAT → container :80`, `HOST_FORWARD_LISTENING` loopback-only, `curl -v http://127.0.0.1:8080` nginx |
| A7 | VirtioFS is sufficient for bind mounts (correctness) | **KNOWN** (Spike 4 PASS core) | Proven `VZVirtioFileSystemDeviceConfiguration` `harpoon-share` `VirtioFS` `fuse`+`virtiofs` modular, `mount -t virtiofs harpoon-share /mnt/harpoon-share` `HARPOON_VIRTIOFS_RW_OK`, `docker -v /mnt/harpoon-share:/workspace` read/write, Git `status` detects host changes; `inotify` host→guest not observed (informational) |
| A8 | Memory reclamation via virtio balloon (`VZVirtioTraditionalMemoryBalloonDeviceConfiguration`) | **KNOWN** (Spike 5) | Configured `VZVirtioTraditionalMemoryBalloonDevice` + guest `virtio_balloon.ko 36K` `HARPOON_BALLOON_DRIVER_OK`, `HARPOON_BALLOON_TARGET_REQUEST/SET/APPLIED` via `/tmp/harpoon-control`, guest `MemAvailable` reclaimed `815→555→806 MiB` (PASS), host `VM XPC` footprint `NOT OBSERVED` to shrink (`507→604→627 MB` after `768` target) — `BALLOON_GUEST_RECLAIM PASS`, `BALLOON_HOST_FOOTPRINT_REDUCTION NOT OBSERVED` — do not market balloon as proven host reclamation |
| A9 | Explicit `drop_caches` before balloon inflation | **EXPERIMENTAL** — downgraded from asserted ordering | See `memory-model.md` challenge. Assumed useful without measurement. |
| A10 | Rust as primary daemon language with optional tiny Swift bridge | **SUPPORTED** | Rust preferred for daemon/policy; Apple framework demands ObjC runtime. Minimal Swift helper is acceptable if it reduces fragility. |

### Docker Transport Alternatives (to be evaluated in Spike 2)

MVP requirement is Docker API compatibility; transport is an implementation choice. Candidates:

1. **vsock** (`VZSocketDevice` + `VZSock`): virtio vsock. Pro: VM-isolated, no host TCP port; Con: requires guest vsock support, framing.
2. **TCP over VM NAT** (`VZNATNetworkDeviceAttachment` + `VZVirtioNetworkDeviceConfiguration`) with host Unix-socket → TCP proxy: Pro: standard networking; Con: port management, extra hop, possible firewall/VPN interaction.
3. **Virtio filesystem socket relay** (expose `docker.sock` via VirtioFS): **REJECTED for MVP** — `architecture.md` already forbids shared-filesystem socket dependency; keep as non-goal.

Evaluation criteria for Spike 2: latency, throughput, concurrent connections, error handling, code complexity. Document choice in ADR.

### Networking Boundary Responsibility

```
container
  ↓  veth → docker bridge (docker owns)
Linux VM (netns, iptables/nftables, bridge)
  ↓  virtio-net → VZNATNetworkDeviceAttachment  (Harpoon owns VM↔macOS boundary)
VM network boundary (NAT)
  ↓  Harpoon host port-forward / NAT helper  (Harpoon owns)
macOS localhost:<port>  (observed by developer)
```

Harpoon explicitly owns only the VM↔macOS boundary, not container bridge/DNS. Whether `docker -p` automatically appears on `localhost` without a Harpoon helper is **KNOWN** — Spike 3 proved host forward required (`127.0.0.1:8080` `Harpoon host TCP forward`).

### VirtioFS Correctness Requirements (not performance) — Spike 4 PASS core, inotify informational

Spike 4 verified:

- file watchers (`inotify`/`FSEvents` propagation) and hot-reload,
- symlinks (preserved, not dereferenced incorrectly),
- permissions (`chmod`/`chown` visibility host↔guest),
- UID/GID mapping (host user vs guest `root`/container user),
- Git repository integrity (`.git/` with many small files, `git status` speed/correctness),
- Node dependency trees (deep, many symlinks),
- case sensitivity (macOS APFS case-insensitive by default vs Linux ext4 case-sensitive),
- concurrent file changes (host and container writing same bind mount).

Performance optimization deferred until correctness basis exists — host↔container `cat`/`echo` and `Git status` passed, `xt_*`/`bridge` semantics proven.

### Spike 5 Memory Behavior Summary (see docs/results/SPIKE5.md)

- Harpoon `1024 MiB` configured, guest `~970 MiB` visible, idle `VM XPC` `~360-386 MB`; `768 MiB` guest `~718 MiB` idle `~367 MB`; `512 MiB` guest `~468 MiB` idle `~350 MB` — all functional, `1024 MiB` sensible default (`768` no benefit, `512` only ~10-18 MB saving).
- Demand-backed growth `386 → 919 MB` under `nginx+Redis+Postgres`, high-water `919 MB` retained `+5s/+30s/+60s` (natural reclamation NOT OBSERVED).
- Balloon `GUEST_RECLAIM PASS` (`MemAvailable` 815→555→806) but `HOST_FOOTPRINT_REDUCTION NOT OBSERVED` (`507→604→627 MB` after `768`), not marketed as reclamation.

### Memory Distinctions (see memory-model.md) — Spike 5 qualification

Configured capacity vs guest physical vs guest active vs page cache vs reclaimable vs ballooned vs cgroup vs host resident (`phys_footprint`) vs macOS compressed vs swap vs memory pressure — reclamation success is **host resident decrease**, not guest free increase. Spike 5 qualified benchmark: same Mac, snapshot window `+5s` stable (`+30s/+60s` high-water NO reclamation observed), `2` vCPUs vs Docker Desktop `11` vCPUs, `~970` vs `~7922 MiB` visible, `nginx+Redis+Postgres` equivalence with `--dns 1.1.1.1 8.8.8.8` `compat` — figures are measurement qualified, not universal guarantees.


## Distribution (M11)

Relocatable, self-contained macOS installation — no source required after install.

- **Layout**: `bin/harpoon` (802K arm64, `com.apple.security.virtualization`), `lib/harpoon/Image-virt` (33M), `lib/harpoon/harpoon-initramfs.cpio.gz` (14M), `lib/harpoon/harpoon-root.img` (2.0G logical / 962M allocated sparse) — staged at `dist/harpoon-0.1.0-dev-darwin-arm64`, archived `*.tar.gz` (289M) + `.sha256`.
- **Resolution**: `RuntimeConfig.installedLibDir` checks `/usr/local/lib/harpoon`, `/opt/homebrew/lib/harpoon`, `bin/../lib/harpoon` (relocatable), then cwd fallback. `resolveRootDisk` prefers `~/Library/Application Support/Harpoon/data/harpoon-root.img` then `/tmp/harpoon-runtime/data/harpoon-root.img` (sandbox fallback), provisioning via `cp -c`/`ditto` clone (fixes `FileManager.copyItem` 36M truncation).
- **Immutability**: `/usr/local/lib/harpoon` immutable template; user disk mutable and preserved across reinstall/uninstall (uninstall `--purge` opt-in only).
- **Install**: `harpoon/install.sh` (checks `harpoon status`/`lsof /tmp/harpoon.lock`, refuses if running), `harpoon/uninstall.sh` (preserves user data, removes harpoon docker context if owned).
- **Security**: virtiofs/port/balloon preserved, sockets `0600`, `flock` ownership, no TCP exposure, `--purge` path-validated.
- **Verification**: `harpoon doctor` (11 PASS), `codesign --verify`, `shasum -c`, `spctl` (ad-hoc → not notarized, expected).
- See [Installation](installation.md).

## Open Questions

These must be resolved empirically before crate boundaries are frozen:

- vsock framing vs. alternative IPC for the Docker socket bridge (latency/throughput).
- VirtioFS bind-mount performance baseline vs. alternatives.
- Balloon responsiveness characteristics under real workloads.

