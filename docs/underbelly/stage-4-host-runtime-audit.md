# Stage 4 — Host Runtime Audit

Status: `STAGE_4_GATE_PASS` for the audit barrier. This document records the current implementation; it does not authorize or contain a refactor.

## Scope and evidence

Audited the Swift host runtime at `HEAD=642d91d`: `Lifecycle`, `VMManager`, `RuntimeConfig`, `HarpoonRuntimeState`, all bridge types, port forwarding, lifecycle CLI, `main.swift`, and existing lifecycle/regression harnesses. Stage 0–1, Stage 2, and Stage 3 were treated as PASS preconditions and are rerun by the Stage 4 gate where the host permits it.

QMD historical retrieval was attempted with the repository-local `.qmd/index.sqlite` and failed with `SQLITE_CANTOPEN`; no historical claim below relies on that unavailable result. Current source and prior gate artifacts are the evidence of record.

## Authoritative ownership map

| Responsibility | Current authority | Evidence | Assessment |
|---|---|---|---|
| In-process lifecycle transitions | `Lifecycle.state` and `Lifecycle.transition` | `harpoon/Sources/Lifecycle.swift:3-27` | `GOOD_BOUNDARY_KEEP`; writes are centralized and lock-protected |
| VM object/configuration | `VMManager` | `VMManager.swift:57-154` | `GOOD_BOUNDARY_KEEP` |
| Guest Docker readiness | `VMManager.bootReady`, set by serial scan | `VMManager.swift:157-172`; `main.swift:269-291` | `DUPLICATED_AUTHORITY_REVIEW`: serial marker, Lifecycle state, socket, and log are separate projections |
| Guest management readiness | `VMManager.mgmtReady` | `VMManager.swift:164-166` | `DUPLICATED_AUTHORITY_REVIEW`: observed but not required for `.running` |
| Host “running” status | `statusSnapshot()` | `HarpoonRuntimeState.swift:337-380` | `DUPLICATED_AUTHORITY_REVIEW`: reconstructs from PID/process identity, flock, socket, connect, and log |
| VM start/stop lifecycle | `main.swift` + `VMManager` | `main.swift:229-334` | `LIFECYCLE_COUPLING_REVIEW`: signal/timers in top-level process, resource teardown in manager/bridges |
| Disk provisioning/growth | `RuntimeConfig` and disk CLI | `RuntimeConfig.swift` disk helpers; `HarpoonDiskCLI.swift` | `GOOD_BOUNDARY_KEEP`; host owns image geometry, Docker owns guest data |
| Docker and management Unix endpoints | `BridgeSet` extensions | `Bridges.swift:4-104`, `UnixBridge.swift:7-36`, `MgmtBridge.swift:6-36` | `GOOD_BOUNDARY_KEEP` with ownership bookkeeping |
| Published host ports | `PortForwardManager` | `PortForwardManager.swift:43-65,188-323` | `POLLING_RECONCILIATION_MEASURE` |

The machine-readable form is [stage-4-host-runtime-ownership.json](stage-4-host-runtime-ownership.json).

## State transitions and readiness

The foreground process writes the transition sequence `STOPPED -> STARTING -> BOOTING -> DOCKER_READY -> RUNNING`; shutdown writes `STOPPING -> STOPPED`, and configuration, VZ start, guest readiness timeout, or Docker failure write `FAILED`. `Lifecycle` is therefore authoritative only inside that process. It is not persisted and is not queried by `status`.

`VMManager.checkSerial()` is the guest observation point. `HARPOON_DOCKER_READY` sets `bootReady`; the 1-second main-queue timer then advances lifecycle and starts bridges. `HARPOON_MGMT_READY` sets `mgmtReady`, but management readiness does not gate `.running`. A later Docker unhealthy or VM-exited marker has no dedicated state transition path in the host code audited.

The CLI’s external “running” answer is inferred: a live Harpoon PID or held flock is combined with Docker socket existence/readiness and a `HARPOON_RUNNING` log fallback. This is useful for detached CLI operation but duplicates the foreground authority and can report `starting`, `stale`, or `degraded` without a persisted transition record.

## Socket, FD, and listener lifecycle

| Resource | Creation/owner | Permission | Teardown | Gap |
|---|---|---:|---|---|
| `/tmp/harpoon-docker.sock` | `BridgeSet.startUnixBridge()` | `0600` | `BridgeSet.stopAll()` cancels listener source; its cancel handler closes listener FD; owned pathname is unlinked | Per-client FDs and vsock connections live in closure-local sources; shutdown does not registry-cancel them directly |
| `/tmp/harpoon-mgmt.sock` | `BridgeSet.startMgmtBridge()` | `0600` | Same pattern via `mgmtListenerSource` and `ownsMgmtSocket` | Same closure-local connection tracking gap |
| `/tmp/harpoon-control` | `startBalloonControl()` | `0600` | listener cancel handler closes/unlinks; `stopAll()` cancels client sources and closes client FDs | Cancel + explicit close is dual ownership and should remain under review |
| Published TCP listeners | `PortForwardManager.addForward()` | loopback bind | `Forward.source` cancel handler closes FD; `stopAll()` cancels all forwards | Active accepted client FDs are closure-local and not globally tracked |
| VZ vsock connection FDs | `VZVirtioSocketDevice.connect` callbacks | VZ-managed | `conn.close()` in bridge close paths; `PortForwardManager` closes on main after background read | No central inventory for abrupt stop |
| `/tmp/harpoon.lock` | `main.swift` `open` + `flock` | `0600` | FD close/process exit releases flock; pathname intentionally remains | stale pathname is harmless but external status probes it |
| runtime metadata/log | lifecycle CLI | app support, log `0600` | stop removes pid/json; log is rotated, not removed | abrupt crash can leave metadata until next probe |

Stale cleanup is connect-probe based for Unix sockets: a live endpoint is treated as another runtime; an unconnectable pathname is unlinked before bind. The host process does not blindly unlink a socket it did not mark owned. This is a good safety boundary, but cleanup is not atomic with another process racing between probe and bind.

## Queue and blocking model

The main queue owns VZ calls, lifecycle transitions, signal sources, readiness/stop-file timers, Unix listener accepts, balloon control, port reconciliation, and published-port listener accepts. Per-connection Docker and management byte forwarding runs on global queues. Port-forward guest `connect()` and proxy I/O run globally; VZ connect and VZ connection close are marshalled to main.

Blocking operations found:

- `Data(contentsOf:)` scans the serial log on the 1-second main timer and during guest-IP discovery.
- `Process.waitUntilExit()` is used for host DNS, disk provisioning, and CLI/status helpers; lifecycle CLI polling uses `Thread.sleep` in its foreground command process.
- Docker/management bridge write loops retry with `usleep(1000)` on `EAGAIN`; this can occupy a global queue worker.
- Management’s first-request translation temporarily uses a 2-second receive timeout; Docker HTTP translation can therefore block its connection worker.

The split is serviceable for the current foreground runtime, but the blocking boundaries should be measured before any architectural rewrite.

## Timers and pollers

| Poller/timer | Cadence | Trigger/fallback |
|---|---:|---|
| Serial readiness | 1s main | Docker ready starts bridges; Docker failure initiates shutdown |
| Stop-file | 1s main | `/tmp/harpoon-stop` requests normal shutdown |
| Boot timeout | one-shot, 120s default | guest readiness timeout -> FAILED -> shutdown |
| Guest IP discovery | 1s, max 15s | serial marker; fallback `192.168.64.3` |
| Port reconciliation | 10s after 5s initial delay | fallback correctness window; event-triggered delayed sync at 200ms–3s |
| VZ stop watchdog | one-shot 8s | cleanup and force exit if stop callback does not complete |
| CLI start/stop polling | 0.2–0.5s loops, bounded 10–60s | waits for child, lock, and socket state |

The 10-second port poll is explicitly justified in source as an idle fallback; it is the clearest `POLLING_RECONCILIATION_MEASURE` candidate, not evidence for a daemon.

## Failure and recovery map

- Partial configuration/build failure: lifecycle fails before VM start and process exits. VZ start failure logs `HOST_VZ_START_FAILURE`, cleans ephemeral bridge state, and exits.
- Partial bridge startup: `BridgeSet.startAll()` invokes four starts without a transaction. Individual bind/listen failures log and return; the top-level lifecycle still advances to `RUNNING` after `startAll()`. This is `LIFECYCLE_COUPLING_REVIEW` and the strongest current implicit failure boundary.
- Guest Docker failure/timeout: serial observation sets failure or timeout; host transitions to `FAILED`, initiates shutdown, and bounded exit follows.
- Normal stop: SIGTERM/SIGINT or stop-file transitions to `STOPPING`, cancels pollers/bridges, requests VZ stop, then cleans ephemeral paths and exits. Stop waits 10 seconds externally and refuses force-kill.
- Restart: `handleRestart` runs stop, waits 0.5 seconds, then starts; persistent disk/config are retained.
- Abrupt host crash: kernel closes FDs and releases flock, but Unix pathname, PID/JSON, and log can remain. Next start/status uses process, connect, and stale-file probes. No crash journal or recovery coordinator exists.
- Unexpected VM exit after successful start: no dedicated VZ state-change/delegate path was found. The host can continue presenting listeners until I/O fails, and external status relies on socket/process/log probes. This is an explicit gap, not a claimed live observation.

Existing M7/M12/M15/M17/M18 harnesses cover stale metadata, failed start, socket cleanup, restart, and Docker recovery. Stage 4 reuses only their already-supported stop/start and stale-state probes; it does not inject a new destructive crash scenario.

## Classifications and boundary judgment

`GOOD_BOUNDARY_KEEP`: Docker Engine/containerd/runc remain guest-owned; the Swift host is a VZ/Unix/vsock boundary owner. `Lifecycle`/`VMManager`/`BridgeSet` are recognizable ownership seams.

`DUPLICATED_AUTHORITY_REVIEW`: in-process Lifecycle, VMManager booleans, serial markers, runtime metadata, flock, socket probes, and log markers overlap. This is a future contract/diagnostics concern, not a Stage 4 refactor.

`IMPLICIT_OWNERSHIP_REVIEW`: accepted client FDs, vsock connections, and port-forward client connections are closed through local closures rather than one centrally inspectable registry. Normal paths have close logic; crash/stop observability is weaker.

`BLOCKING_IO_REVIEW`: main-queue serial reads and synchronous Process/file waits deserve measurement if boot or UI responsiveness becomes a problem.

`HISTORICAL_OR_DEAD_CANDIDATE_REMOVE`: legacy host-forward fields and `_legacy_startHostPortForward` are retained for compilation/reference and are not live ownership paths.

### Confirmed implementation risks (audit findings, not fixes)

- `BridgeSet.startAll()` has no aggregate result. Individual bridge bind/listen failures are log-only, while `main.swift` still advances to `RUNNING` and emits `HARPOON_RUNNING`. This is a confirmed false-readiness path (`LIFECYCLE_COUPLING_REVIEW`).
- The runtime bridge reads `RuntimeConfig.dockerSocketPath`, while launcher/status helpers use fixed `HarpoonPaths.dockerSocketPath`; a custom socket configuration can therefore split runtime ownership from CLI readiness (`DUPLICATED_AUTHORITY_REVIEW`).
- Socket permission checks are permissive in current code: `socketExists0600()` returns true regardless of mode and launcher success uses `okPerms || true`. Live evidence happened to show `srw-------`, but the source does not enforce that contract (`IMPLICIT_OWNERSHIP_REVIEW`).
- Active Docker, management, and published-port connection sources are not registered in central owner collections, so `stopAll()` directly controls listeners but not every accepted connection (`IMPLICIT_OWNERSHIP_REVIEW`).
- Nonblocking proxy writes retry with `usleep(1000)` on `EAGAIN`; a stalled peer can occupy a global worker indefinitely (`BLOCKING_IO_REVIEW`).
- `Lifecycle` accepts arbitrary transitions and its state read is not synchronized through the same lock; it is a transition logger/state holder, not a validated state machine (`DUPLICATED_AUTHORITY_REVIEW`).

## Persistent daemon decision

Evidence does not justify a persistent host daemon now. The current foreground child already owns the VM, VZ devices, lock, bridges, timers, and cleanup; the CLI launcher provides detached start/stop/status semantics with bounded polling. A daemon would add another authority and crash/reconnect boundary while the present gaps are duplicated state, untracked connection inventories, partial bridge startup semantics, and missing unexpected-VM-exit observation. Measure those gaps or demonstrate a concrete requirement for runtime survival independent of the foreground process before reconsidering.

## Exit criteria

All host-side long-lived components, sockets, timers, queues, readiness sources, and failure paths are mapped above and in the JSON map. No Swift production file, Docker Engine/containerd/runc component, or behavior-bearing runtime path was changed for Stage 4.
