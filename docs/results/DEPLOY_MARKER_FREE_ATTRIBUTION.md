# Marker-free application health checks with verified workload attribution

Ordinary applications (for example Harpoon Hello) do not echo `X-Harpoon-Deployment-Marker`, so a live deployment could never pass its health check. This change lets such an application pass **only when the response path is proven**, and keeps the marker as the preferred, stronger evidence.

## Invariant

> A health-check success is attributable to the exact container and release Harpoon created and verified, through a path Harpoon proved leads to the VM it manages.

Anything unproven fails closed. There is no weaker success condition.

## Tiers

| Response | Result | Evidence label |
|---|---|---|
| Carries the container's marker | success | `marker-verified` |
| Carries a **different** marker (stale or wrong) | failure, the proof is never consulted | — |
| 2xx without a marker, **and** the full proof holds | success | `path-verified` |
| 2xx without a marker, proof absent or failing | failure with the reason | — |

Two consecutive successful probes are still required. The same `health()` serves the preliminary, final and restoration checks, so rollback and restoration get the same rule. The label is a `[label]` suffix on `CheckEvidence.expected`; there is no journal schema change.

## Proof chain

1. **Host listener.** Exactly one loopback listener on the leased port, owned by the forwarder pid (existing `lsof` check).
2. **Forwarder channels.** The same pid holds the Docker socket and the management socket (`lsof -U`), so the engine whose sockets we use is the process that owns the listener.
3. **VM identity, not IP equality.** The engine publishes `forwardTarget` in `runtime.json` (a *discovered* address only; the hard-coded fallback is never published and clears the field). The CLI sends a fresh 128-bit challenge through the vsock-bound management `exec` to a one-shot guest listener on TCP 2378 and reads it back over TCP at the published address. Only the VM behind the management channel can have received the challenge, so a matching answer proves the machine at that address is the engine's VM.
4. **Route.** Docker's mapping decides the route. An unspecified `HostIp` (ordinary guest-address forwarding) is covered by step 3. `127.0.0.1` (the PR #157 vsock relay) is covered by checking that the guest relay for vsock port `65536+P` exists; vsock is VM-bound by construction. A port published both ways, or any other address, is refused.
5. **Workload identity.** Container id, image id (equal to the image Harpoon created), `StartedAt` and `RestartCount` are recorded at the first running attempt and must be unchanged afterwards; the Docker port mapping equals the lease before and after.
6. **Bracketing.** The path proof runs at the first accepted unmarked response and again after the final success.

## Limits

- **TOCTOU.** Between the last proof and the last response there is a millisecond-scale window; a container swap inside it is caught by the baseline comparison afterwards, but a response in that window is not retroactively invalidated beyond that.
- **Hostile peer on the NAT segment** that races the VM for the address is out of scope for the TCP challenge; the vsock-bound challenge and relay route do not depend on it.
- An application behind a guest-local reverse proxy, or one that serves from a *different* container of the same release, is not distinguished by the path.
- Old engines, or a CLI run without `--forward-address`, keep the strict marker-only behavior.

## Engine restart

The Swift change (`ForwardTarget.swift`, `PortForwardBridge.swift`, `HarpoonRuntimeState.swift`) must be built into the running engine for it to publish `forwardTarget`. **A controlled engine restart is required eventually**; until then a running engine stays strict and plain applications still fail. Nothing here restarts or modifies the serving engine.

## Interfaces

- CLI: `--forward-address IPV4` on `deploy`, `replace`, `rollback`, `restore` and the Git commands (forwarded). Private dotted IPv4 only.
- Desktop: `RuntimeInfo.forward_address` is taken from `runtime.json` `forwardTarget` only when `source == "discovered"` and the address is a private dotted IPv4.
- Code: `deploy-docker/src/attribution.rs` (proof), `probe.rs` (baseline and tiers), `backend.rs` (`PathGate`, `health()`).
