# Stage 6 — Docker Boundary Audit

**Status:** `STAGE_6_GATE_PASS` (2026-10-13/14 America/Chicago)

This is an evidence audit, not a refactor. Docker Engine, containerd, runc,
the Linux kernel, Virtualization.framework, Swift host runtime, guest init,
and UI behavior are unchanged.

## Decision

Docker remains authoritative for the Docker API and all Docker-owned resources:
containers, images, networks, volumes, container lifecycle, published-port
intent, Compose, BuildKit, logs, exec, and container health. containerd/runc and
the Linux kernel remain authoritative below Docker. Harpoon owns only the
macOS↔Linux boundary: VM lifecycle, Unix/vsock transport, host path transport,
host loopback forwarding, disk envelope, host diagnostics, and guest
management.

No unaccounted competing Harpoon state model for Docker objects was found.
Harpoon keeps only reconstructible boundary state: sockets, port-forward file
descriptors, translated paths, and UI observation/cache data.

## Canonical ownership matrix

| Domain | Authority | Interaction | Competing state | Classification | Exact evidence |
|---|---|---|---|---|---|
| API/socket | Docker Engine | transport, translate, observe | No | `REQUIRED_BOUNDARY_TRANSLATION` | `harpoon/Sources/UnixBridge.swift:7-181`; `tools/guest-builder/src/init:729-878` |
| Containers/lifecycle | Docker Engine | invoke, observe | No | `THIN_CLIENT_CONTROL` | `harpoon/Sources/ContainerCLI.swift:198-208`; `ui/harpoon-desktop/src-tauri/src/main.rs:448-503` |
| Images | Docker Engine/registry | invoke, observe | No | `THIN_CLIENT_CONTROL` | `ui/harpoon-desktop/src-tauri/src/main.rs:522-543` |
| Networks/topology | Docker Engine/Linux kernel | invoke, observe, translate | No | `SAFE_OBSERVATION` | `ui/harpoon-desktop/src-tauri/src/main.rs:569-591`; `tools/guest-builder/src/init:859-878` |
| Published ports | Docker Engine intent; Harpoon host listener | observe, translate, mutate | Transient forwarding map only | `REQUIRED_BOUNDARY_TRANSLATION` | `harpoon/Sources/PortForwardManager.swift:67-186`; `PortForwardBridge.swift:210-320` |
| Bind mounts | Docker guest↔container; Harpoon macOS↔guest path | translate, observe | No | `REQUIRED_BOUNDARY_TRANSLATION` | `harpoon/Sources/UnixBridge.swift:65-73,129-181`; `HostPathTranslator.swift:1-220` |
| Named volumes | Docker Engine/guest data root | invoke, observe | No | `THIN_CLIENT_CONTROL` | `ui/harpoon-desktop/src-tauri/src/main.rs:545-567`; `tools/guest-builder/src/init:673-734` |
| Disk and `/var/lib/docker` | Harpoon disk envelope; Docker data | mutate boundary, observe | No | `REQUIRED_BOUNDARY_TRANSLATION` | `HarpoonDiskCLI.swift`; `tools/guest-builder/src/init:232-289,673-728` |
| Compose/BuildKit | Docker plugins | invoke, observe | No | `THIN_CLIENT_CONTROL` | `ui/harpoon-desktop/src-tauri/src/main.rs:636-647`; `harpoon/m8-test.sh:150-166`; `harpoon/m9-test.sh` |
| Context | Docker CLI | invoke/setup, observe, cache | Setup policy duplicated | `DUPLICATED_SEMANTICS_REVIEW` | `HarpoonCLI.swift`; `main.rs:692-732`; `harpoon/m8-test.sh:46-65` |
| Logs | Docker container logs; Harpoon VM log | invoke, observe | No | `THIN_CLIENT_CONTROL` | `main.rs:504-513`; `ContainerCLI.swift:88-109` |
| Exec/shell | Docker container exec; Harpoon guest management for guest shell | invoke, transport | No | `THIN_CLIENT_CONTROL` | `ContainerCLI.swift:111-116`; `tools/guest-builder/src/harpoon-mgmt:19-44,197-286` |
| Readiness/restart | Harpoon boundary; Docker daemon/container health | observe, invoke, translate | No Docker state; policy split | `DUPLICATED_SEMANTICS_REVIEW` | `tools/guest-builder/src/init:729-878`; `HarpoonLifecycleCLI.swift:160-218` |
| Stats/attribution | Docker/Linux truth plus Harpoon host diagnostics | observe, cache | No merged truth model | `SAFE_OBSERVATION` | `main.rs:599-625`; `docs/underbelly/stage-2-resource-attribution.json` |

## Required answers

1. Harpoon does not authoritatively own container lifecycle. CLI/UI lifecycle
   operations call Docker; Harpoon lifecycle controls the VM and boundary.
2. No competing image/network/volume/container metadata store was found.
   Port mappings and UI JSON are disposable observations/boundary state.
3. `PortForwardManager` is translation, not Docker networking: it reads
   Docker's `PublicPort`, `PrivatePort`, protocol, address, and identity,
   binds a host loopback listener, and connects to the guest published port.
   Docker owns namespaces, NAT, and publication. UDP is explicitly deferred
   (`PortForwardManager.swift:159-163,210-213`). The source audit also found
   that a changed guest IP with the same host-port key is not recreated
   (`PortForwardManager.swift:188-198`), and duplicate desired keys collapse to
   one mapping (`:168-172`); both are Stage 9 validation debt, not Docker
   authority.
4. Bind mounts are cleanly split: Harpoon translates macOS paths to guest
   `/mnt/harpoon-host/...` during `containers/create`; Docker owns the guest
   bind mount. Named volumes stay in Docker's guest data root.
5. Explicit `--context harpoon` is the client path and Docker responses are
   truth. Swift and Tauri both implement setup/validation, which is a Stage 9
   duplicated-policy finding. The Tauri fallback can remove/recreate a foreign
   `harpoon` context after update failure (`main.rs:714-727`), unlike the safer
   Swift refusal path; this was not exercised because it would mutate user
   Docker configuration. UI cache is not canonical.
6. Compose and BuildKit are delegated to Docker CLI plugins; Harpoon does not
   interpret plans or build graphs.
7. Container logs/exec use Docker semantics. Explicit `harpoon exec -- ...`
   and `harpoon shell` target the guest through management vsock; the CLI
   parser keeps these paths distinct.
8. Diagnostics keep host, guest, and workload cgroup domains separate. Docker
   inspect supplies identity; cgroup files supply workload usage. Two UI risks
   remain unaccounted debt: `get_counts` converts Docker query errors to empty
   counts (`main.rs:607-613`), and the UI's config-path projection differs from
   Swift's Application Support path (`main.rs:315-337` vs
   `HarpoonRuntimeState.swift:21-57`). Neither is authoritative Docker state,
   but both can mislead observation and belong in Stage 9.
9. Readiness is legitimately split between Harpoon VM/boundary readiness and
   Docker daemon/container health. Guest init and host lifecycle both probe
   Docker readiness; this is flagged for Stage 9 policy cleanup, not changed.
10. Architectural invariant:

   > Harpoon may transport, observe, translate, and invoke Docker; it must not
   > infer or persist a second authoritative model of Docker containers,
   > images, networks, volumes, builds, Compose, or container health. Harpoon
   > state must be boundary state and disposable or reconstructible from
   > Docker/Linux truth.

## Live validation and evidence limits

The Stage 6 gate reruns the existing Stage 0–5 barriers with Node 20 and then
uses one uniquely named Alpine fixture to verify Docker create → start →
inspect → logs → exec → stop → rm. Cleanup is restricted to that name.
Existing M8/M9 evidence covers context/socket behavior, named-volume
persistence, BuildKit/buildx, bind-mount translation, and published-port
reachability. No new destructive scenario or production behavior was added.

## Stage 9 queue / explicit non-findings

Review the bridge's narrow HTTP parsing/hijack path, duplicated context setup,
foreign-context fallback, stale guest-IP/duplicate-port reconciliation,
split readiness/retry policy, error-to-zero UI counts, config-path projection,
and presentation-only Compose label enrichment.
Stage 5's `pkill -f`, listener ownership, and management-output findings remain
unchanged. None is a Stage 6 refactor.

All Stage 0–1 through Stage 5 gates, Stage 6 static checks, and the owned live
Docker path passed. No Stage 7 or UI work started. Formal stop:
`STAGE_6_GATE_PASS`.
