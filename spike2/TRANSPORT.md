# Spike 2 Transport — Corrected Evaluation (2026-08-25)

## Error in Previous TCP Proposal

Previous `spike2/build.sh` used:

```sh
dockerd --host=tcp://127.0.0.1:2375
# host proxy: socat UNIX-LISTEN:/tmp/harpoon-docker.sock -> TCP:192.168.64.2:2375
```

`127.0.0.1` is guest `lo` only. Host `192.168.64.x` via `VZNATNetworkDeviceAttachment` cannot reach guest `lo`. `curl 192.168.64.2:2375` would be `connection refused`. This is a reachability error.

Corrected TCP (if used) must bind to guest `eth0` address, not `127.0.0.1`:

```sh
# guest
IP=$(ip -4 addr show eth0 | awk '/inet /{print $2}' | cut -d/ -f1) # 192.168.64.x via VZNAT DHCP
dockerd --host=unix:///var/run/docker.sock --host=tcp://$IP:2375
# or --host=tcp://0.0.0.0:2375 with firewall restricting to 192.168.64.0/24
# host proxy: socat UNIX-LISTEN:/tmp/harpoon-docker.sock -> TCP:$IP:2375
```

Host can reach `192.168.64.x` through `VZNATNetworkDeviceAttachment` because `VZNAT` creates a NAT network where host and guest share `192.168.64.0/24`; guest `eth0` gets `192.168.64.x` via `udhcpc`, host sees it via VM NAT. Still requires IP discovery (parse serial or `VM IP` via `VZ`).

## Corrected Comparison

### Option A — Virtio socket / vsock (PREFERRED for spike)

```
macOS Docker client
  ↓ DOCKER_HOST=unix:///tmp/harpoon-docker.sock (0600)
host proxy (socat vsock or Rust harpoon-bridge)
  ↓ AF_VSOCK CID 2 (host) -> CID guest (3) port 2375
guest vsock listener (socat VSOCK-LISTEN:2375 -> UNIX-CONNECT:/var/run/docker.sock)
  ↓
/var/run/docker.sock -> Docker Engine
  ↓ containerd
```

- **No guest IP discovery**: vsock uses `CID` (host `2`, guest `3+` per `VZVirtioSocketDeviceConfiguration`), not `192.168.64.x` DHCP. Host proxy dials `vsock:3:2375` directly, no parsing `udhcpc` lease.
- **No plaintext Docker TCP on eth0**: Docker never listens on `eth0:2375` TCP. Guest `vsock` is VM-isolated, not host LAN, not `0.0.0.0`. No `iptables` exposure beyond VM boundary.
- **Host Unix socket compatible**: Docker clients expect `unix://` socket; host proxy `UNIX-LISTEN` satisfies `DOCKER_HOST=unix://...`.
- **Fewest moving parts for spike**: `VZVirtioSocketDeviceConfiguration` (one device, already proven in Spike 1 `VZNAT`+`entropy` add), `socat` on both sides (`socat VSOCK-LISTEN:2375,fork UNIX-CONNECT:/var/run/docker.sock` in guest, `socat UNIX-LISTEN:/tmp/harpoon-docker.sock,fork VSOCK-CONNECT:3:2375` on host) — no `eth0` IP, no NAT port forward, no DHCP.
- **Cons**: Requires `vsock` kernel support (`CONFIG_VSOCKETS=y`, Alpine `virt` kernel has it; Spike 1 `Image-virt` already `virt`), `socat` with `vsock` support in guest (Alpine `socat` has `vsock`).

### Option B — TCP via VZNAT (corrected)

```
host Unix socket -> host proxy (socat TCP) -> guest eth0 TCP 2375 -> Docker Engine
```

- Must bind `dockerd --host=tcp://$IP:2375` where `$IP` is guest `eth0` `192.168.64.x` (not `127.0.0.1`), or `0.0.0.0:2375` with `iptables -A INPUT -s 192.168.64.1 -p tcp --dport 2375 -j ACCEPT` + default drop.
- Host reaches via `VZNATNetworkDeviceAttachment` NAT `192.168.64.0/24` — documented `VZNAT` is host-only NAT, not bridged, still host-reachable.
- Requires IP discovery (guest `ip addr` → serial → host proxy) and exposes Docker TCP on guest network (even if NAT-restricted, still plaintext TCP).

### Option C — VirtioFS socket relay — REJECTED

Forwards `docker.sock` via shared filesystem — forbidden, couples VM boundary to filesystem.

## Decision for Spike 2

**Prefer vsock (Option A) for smallest proof**: host `UNIX-LISTEN` without IP discovery, no `eth0` TCP listener, fewest parts, VM-isolated. Keep `VZNAT` for guest `apk` internet, but not for Docker transport.

Guest change vs previous `spike2/build.sh`:

```sh
# was: dockerd --host=tcp://127.0.0.1:2375 (wrong)
# now (vsock):
apk add socat  # vsock support
# start vsock -> unix bridge in guest
socat VSOCK-LISTEN:2375,fork UNIX-CONNECT:/var/run/docker.sock &
# dockerd only on unix socket (no TCP)
dockerd --host=unix:///var/run/docker.sock --bridge=none --iptables=false > /dev/hvc0 2>&1 &
# host proxy (spike, Python/Rust):
# socat UNIX-LISTEN:/tmp/harpoon-docker.sock,fork VSOCK-CONNECT:3:2375
# or: VZVirtioSocketDevice (host CID 2) -> guest CID 3 port 2375
```

If vsock `socat` not available in Alpine `socat`, fallback corrected TCP (`--host=tcp://$IP:2375`) with documented `VZNAT` reachability, but vsock is first attempt.

## Spike 2 Host↔Guest Transport Provenance

- `VZVirtioSocketDeviceConfiguration` (macOS 11+, `VZSocketDevice` `vsock` `CID` `port`), `socat` `vsock` (Alpine `community/socat`), `harpoon-bridge` (Rust `vsock` crate for prod)
- No new entitlements (`com.apple.security.virtualization` already covers `vsock` device)

## Smallest Next Experiment (vsock)

1. Boot `Image-virt` + `harpoon-docker-initramfs` with `vsock` device (`VZVirtioSocketDeviceConfiguration`) and guest `socat VSOCK-LISTEN:2375` bridge (record `docker --version` `containerd --version` in serial).
2. On host, `socat UNIX-LISTEN:/tmp/harpoon-docker.sock,fork VSOCK-CONNECT:3:2375` (or `harpoon-bridge` `unix -> vsock`).
3. `DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version` -> must show `Server: Alpine` guest `overlayfs` `cgroup2`.
4. `docker run --rm hello-world` inside Harpoon guest.

If vsock `socat` missing, fallback to corrected TCP (`eth0` IP) as second gate.
