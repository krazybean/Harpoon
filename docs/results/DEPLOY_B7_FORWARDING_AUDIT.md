# B7 phase 1 — audit of Harpoon's host-to-guest port forwarding

Offline audit by reading the source (`harpoon/Sources/PortForwardManager.swift`, `PortForwardBridge.swift`, `VMManager.swift`). **Nothing
here was observed on a running Harpoon or VM, and LAN isolation is not proven.** Every claim below is either "the code does X" or an explicit
open question.

## How a published guest port reaches the host
1. **Discovery.** The forwarder runs inside the Harpoon app process. It asks the guest's Docker `GET /containers/json?all=1` over the vsock
   channel (guest port 2375). Only containers with `State == "running"` are considered, and only `Ports[]` entries of type `tcp` with a
   `PublicPort`. Polls: every 10 s (first at 5 s), plus extra syncs 200 ms after the guest IP is learned and at 1 s/3 s after discovery.
2. **Host listener.** For each such port it binds `127.0.0.1:<PublicPort>` — the **same number** on the host as on the guest — with `SO_REUSEADDR`,
   `listen(16)`, non-blocking, accepted on the main dispatch queue. `hostAddrForDockerIP` maps *every* Docker bind address (`0.0.0.0`, `::`,
   `127.0.0.1`, anything else) to `127.0.0.1`: the host listener is loopback-only **by construction**. IPv4 only.
3. **Relay.** Each accepted connection is relayed to `<guest IP>:<PublicPort>` over the VM's NAT network (`VZNATNetworkDeviceAttachment`), byte for
   byte, no TLS, no authentication, no limit on connections.
4. **Guest side.** Harpoon's deployments publish without `HostIp`, so Docker in the guest binds the wildcard (`0.0.0.0` and `::`) on the guest.
   That port is reachable from anything on the VM's NAT segment — the Mac itself and any other peer on that segment.
5. **Test ports (39000–39099) and application ports (38000–38999) use the same path**, so the preliminary and final checks cross the same forwarder.

## Findings that matter for B7
| # | Finding | Consequence | Mitigation (implemented offline / needed live) |
|---|---|---|---|
| F1 | If no `HARPOON_GUEST_IP` appears in the serial log within 15 s the app **falls back to a hard-coded `192.168.64.3`** and forwards there. | Traffic could be relayed to an address that is not Harpoon's guest. | Preflight: the Harpoon log must show `HARPOON_GUEST_IP_DISCOVERED` and neither `HOST_FORWARD_DISCOVERY_FAILED` nor `HOST_FORWARD_TRY_FALLBACK`. *Live check, not automated.* |
| F2 | A failed bind (`HARPOON_PORT_FORWARD_COLLISION`) is only logged and retried at the next sync. An unrelated process then owns the host port. | A health probe on `127.0.0.1:<port>` reaches the wrong program. | Per-container response marker (implemented, tested) **and** the host-listener check (below). |
| F3 | The host listener appears only after the next sync — up to ~10 s after a container starts. | Early probes see "connection refused"; harmless, but a check done too early would fail. | The listener check runs only after a healthy marker response; the 60 s health deadline covers the lag. |
| F4 | `SO_REUSEADDR` on macOS lets the forwarder bind `127.0.0.1:P` while another process holds `*:P`. | Two listeners share a port: loopback traffic goes to Harpoon, LAN traffic to the other process. | Allocation refuses wildcard-held ports (its bind probe binds the wildcard first); the listener check requires **exactly one** listener per port. |
| F5 | The forwarder trusts the guest Docker's listing; there is no per-connection identity check. | A mapping that changes between syncs is followed within ≤10 s. | Container port mapping re-read from Docker before and after the probe and compared with the lease (implemented, tested). |
| F6 | IPv6 is not served. | `[::1]:P` is never Harpoon's. | A listener on `[::1]` is rejected by the check. |

## What must be true, and how it is verified
*Host listener belongs to Harpoon and is loopback-only* — evidence = `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fpcn` shows **exactly one** listener, its address is
`127.0.0.1:<port>`, its pid is the Harpoon app's and its command name matches. Implemented offline as `forwarder::parse_lsof_f` + `verify_forwarder` + `LsofCheck`
(fixed arguments, empty environment, 5 s limit, read-only) and wired into the backend as a check that runs **after a healthy marker response**; the CLI
requires `--forwarder-pid` for a live deployment. Tested with recorded-format `lsof` output and fake `lsof` scripts only — **never against a real host**.

*The response came from this container* — per-container random marker in the container's environment, required in the `X-Harpoon-Deployment-Marker`
response header (see `DEPLOY_DOCKER_BACKEND.md`).

*The path is the leased one* — `NetworkSettings.Ports` for the container equals the lease (no other published port, no specific non-wildcard bind address), checked before and after.

## LAN isolation — not proven
Code reading says the host side is loopback-only and the guest side is on a NAT segment. That is a design statement, not a measurement. A live test needs a second
device on the LAN and the user's authorization (see `DEPLOY_B7_LIVE_ACCEPTANCE.md`): from the second device, connect to `<Mac LAN address>:<port>` for a leased port
(expected: refused) and to the guest NAT address (expected: unreachable); from the Mac, connect to `<guest NAT address>:<port>` (expected: reachable — this documents the
local exposure the design accepts). Until then, **LAN isolation is an open B7 gate.**

## Questions only a live run can answer
- Does `lsof` on the target macOS show the app's listener with the command name and address format the parser expects (the fixtures follow `lsof -F` documentation)?
- What is the Harpoon app's real process name and pid source for `--forwarder-pid`/`--forwarder-command`?
- Does the VM NAT segment accept connections from other hosts on the LAN?
- Is the 10 s sync lag what is observed, and does `HARPOON_PORT_FORWARD_LISTENING` appear before the first successful probe?
