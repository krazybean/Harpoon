# Spike 3 — Container Networking and Localhost Port Publishing

Status: INCOMPLETE — host VZ transient blocks live proof (REBOOT_SKIPPED)

## Host VZ Reliability — Blocking

Date 2026-08-25T03:36Z:
- host Version 26.5.2 Build 25F84 arm64 isSupported=true
- validate OK (VZNAT, VZGenericPlatformConfiguration, VZLinuxBootLoader, VZVirtioSocketDevice, VZVirtioBlockDevice)
- start FAIL VZErrorDomain code=1 Internal Virtualization error state=3 before guest (diag CYCLE_1_START_FAIL)
- Same failure for spike1 Image-virt/initramfs-virt and spike2 Image-virt/harpoon-docker (14M 68411742ace18227fd09c2981a57588b09e316e97a2fb0a02b449bfaad6145d4)
- df: /System/Volumes/Data 460Gi 389Gi 34Gi 92% (was 98% previously), plus simulator volumes 96-98%
- Entitlement com.apple.security.virtualization verified via codesign -d
- Classification: UNRESOLVED HOST/FRAMEWORK STATE ISSUE REBOOT_SKIPPED (non-negotiable)
- Previous Spike 2 PASS was proven before this transient; Spike 3 artifacts are execution-ready and correct, but cannot be live-proven until host recovers naturally (disk cleanup or reboot, previous transient cleared after rm -rf /tmp/harpoon*).

Do not loop VM starts. Manual retry when host recovers: `./spike2/build/harpoon-spike2-vsock` from normal Terminal (no Muse launch).

## Provenance
- Kernel: spike1/cache/Image-virt 33M Alpine 6.12.94-0-virt (uncompressed via gunzip, console=hvc0)
- Initramfs: spike2/cache/harpoon-docker-initramfs.cpio.gz 14M SHA256 68411742ace18227fd09c2981a57588b09e316e97a2fb0a02b449bfaad6145d4 (rebuilt 2026-08-25T03:35Z Spike 3)
- Block: spike2/cache/harpoon-root.img 2G sparse raw ext4 UUID 00000000-0000-0000-0000-000000000001 (pivot_root suitable, not ramdisk)
- Host: spike2/build/harpoon-spike2-vsock 197K SHA db55d1ebe4be5137b302b245ed5d8ee08aa4a28f1f681a005b7d94994f48ffad signed com.apple.security.virtualization
- Modloop: /tmp/modloop-virt 16M SquashFS 2025-05-13 6.12.94-0-virt (source for all injected kmods)
- Repos: https://dl-cdn.alpinelinux.org/alpine/v3.22/main + community (APKINDEX verify via apk policy)

## Phase 1 — Kernel/Module Requirements Discovered (6.12.94-0-virt)

Verified via modloop modules.dep/modules.builtin (134 builtins, none of these are builtin):

- bridge deps: kernel/net/802/stp.ko + kernel/net/llc/llc.ko -> kernel/net/bridge/bridge.ko -> kernel/net/bridge/br_netfilter.ko
- veth: kernel/drivers/net/veth.ko (no deps)
- overlay: kernel/fs/overlayfs/overlay.ko (Docker storage, no deps)
- netfilter base: kernel/lib/libcrc32c.ko, kernel/net/ipv4/netfilter/nf_defrag_ipv4.ko, kernel/net/ipv6/netfilter/nf_defrag_ipv6.ko
- conntrack/nat: kernel/net/netfilter/nf_conntrack.ko -> kernel/net/netfilter/nf_nat.ko
- iptables: kernel/net/netfilter/x_tables.ko -> kernel/net/ipv4/netfilter/ip_tables.ko -> kernel/net/ipv4/netfilter/iptable_nat.ko (+ iptable_filter, iptable_mangle)
- MASQUERADE/addrtype: kernel/net/netfilter/xt_MASQUERADE.ko, xt_addrtype.ko, xt_conntrack.ko, xt_comment.ko, xt_tcpudp.ko (pulled via nf_nat/nf_conntrack)
- All modular (CONFIG_BRIDGE=m, BR_NETFILTER=m, VETH=m, NF_CONNTRACK=m, NF_NAT=m, IP_TABLES=m, IPTABLE_NAT=m, OVERLAY_FS=m) — verified not in modules.builtin, present in modules.dep with deps as above.

Injection: build.sh now copies from /tmp/vsock_modprep (modloop extract) into $ROOT/lib/modules/6.12.94-0-virt: stp, llc, bridge, br_netfilter, veth, overlay, full net/netfilter/*, net/ipv4/netfilter/*, net/ipv6/netfilter/*, ipset, libcrc32c, nf_defrag. Keeps coherent modules.dep from same modloop (depmod unavailable on macOS, copy). ls verifies bridge.ko 462K, veth 53K, overlay 295K, nf_conntrack 278K, nf_nat 81K. grep modules.dep has bridge/veth/overlay.

## Phase 2 — Docker Daemon Networking Changes

Previous Spike 2 constrained: `dockerd --host=unix:///var/run/docker.sock --bridge=none --iptables=false --ip-masq=false --userland-proxy=false` (API-only proof, no bridge/NAT, hence WARNING IPv4 forwarding disabled).

Spike 3 corrected to normal Docker bridge networking:
- Enable `net.ipv4.ip_forward=1` before dockerd via `echo 1 > /proc/sys/net/ipv4/ip_forward` + `sysctl -w net.ipv4.ip_forward=1`, verify `cat /proc/sys/net/ipv4/ip_forward` == 1 else HARPOON_DOCKER_FAILED ip_forward.
- Pre-load modules: stp, llc, bridge, br_netfilter, veth, overlay, libcrc32c, nf_defrag_ipv4/ipv6, nf_conntrack, nf_nat, x_tables, ip_tables, iptable_nat/filter, xt_MASQUERADE/addrtype/conntrack/comment/tcpudp via modprobe (fallback observable). Set `bridge-nf-call-iptables/ip6tables=1`.
- Emit `HARPOON_GUEST_IP <192.168.64.x>` after DHCP for host forwarder (re-emitted after vsock ready).
- Start dockerd as `dockerd --host=unix:///var/run/docker.sock` (defaults: bridge docker0, iptables true, ip-masq true, userland-proxy true — required for -p). No TCP expose, vsock-only API preserved.
- After vsock ready, verify: `ip link show docker0`, `ip addr show docker0`, `docker network ls`, `docker info | grep Bridge`, `iptables -L`/`nft list`, `lsmod | grep bridge`, emit HARPOON_BRIDGE_CHECK/OK/PENDING, HARPOON_IP_FORWARD, HARPOON_NET_READY.

Docker API transport unchanged: VZVirtioSocketDevice :2375 -> socat VSOCK-LISTEN:2375 -> /var/run/docker.sock via host Unix /tmp/harpoon-docker.sock (full-duplex half-close, BRIDGE_* logs).

## Phase 3 — Host Forwarder

Architecture (preferred, spec-compliant):
```
macOS 127.0.0.1:8080 -> Harpoon host TCP forward (loopback-only) -> guest VZNAT IP:8080 -> Docker NAT (iptables DNAT) -> container :80
Docker API: macOS /tmp/harpoon-docker.sock -> vsock :2375 (unchanged)
```

Host Swift `spike2/swift/main.swift` 197K:
- Constants hostForwardPort 8080 guestForwardPort 8080
- State hostForwardFd/Source/GuestIP/Started
- `parseGuestIP()` polls /tmp/harpoon-spike2-serial.log for last HARPOON_GUEST_IP line (192.*), else unknown
- `startHostPortForward(guestIP:)` binds 127.0.0.1:8080 SO_REUSEADDR, listen 16, log HOST_FORWARD_LISTENING loopback-only, non-blocking DispatchSourceRead, nextForwardId
- Per-client: async connect to guestIP:8080 (blocking connect in global queue, timeout via OS), log HOST_FORWARD_ACCEPT/CONNECT_FAILED/CONNECTED, full-duplex proxy 8192B, half-close shutdown(SHUT_WR) both directions, EAGAIN/EPIPE handling, HOST_FORWARD_CLOSE/C CLIENT_EOF/GUEST_EOF, no HTTP parsing
- Discovery: after HARPOON_DOCKER_READY, poll parseGuestIP every 1s, start on discovered IP, log HARPOON_GUEST_IP_DISCOVERED, fallback to 192.168.64.3 after 15s with HOST_FORWARD_DISCOVERY_FAILED/HOST_FORWARD_TRY_FALLBACK, explicit 8080 config (acceptable spike, production would poll docker inspect harpoon-web NetworkSettings.Ports or Docker events).

Loopback-only verified: bind inet_addr("127.0.0.1"), not 0.0.0.0.

## Acceptance Gates — Current Live Status (blocked by VZ start failure)

- [ ] net.ipv4.ip_forward == 1 — code emits HARPOON_IP_FORWARD 1 and fails fast if not 1, but not live-proven (VZ blocked)
- [ ] Docker bridge exists — code checks ip link docker0 -> HARPOON_BRIDGE_OK/PENDING, docker network ls, lsmod bridge, but not live
- [ ] test container receives IP — would be via `docker run --rm alpine:3.22 ip addr` after bridge, not live
- [ ] container can reach Internet — `docker run --rm alpine:3.22 ping -c 1 1.1.1.1` container->docker bridge->guest eth0->VZNAT->Internet, not live
- [ ] container DNS works — `docker run --rm alpine:3.22 nslookup dl-cdn.alpinelinux.org` container resolver separate from guest resolv.conf fallback, not live
- [ ] docker run -p 8080:80 nginx:alpine succeeds — guest would show Docker NAT iptables, not live
- [ ] macOS listener exists on 127.0.0.1:8080 — host code HOST_FORWARD_LISTENING loopback-only, but not live
- [ ] curl http://127.0.0.1:8080 returns nginx content — host TCP forward -> guest:8080 -> container:80, not live
- [ ] Docker API continues via /tmp/harpoon-docker.sock — preserved vsock bridge, not live-proven in this run (VZ blocked, but Spike 2 PASS proven)

All gates are execution-ready in code/artifacts, blocked only by HOST_VZ_START_FAILURE REBOOT_SKIPPED.

## Failure Classification — Current

HOST/FRAMEWORK STATE ISSUE (VZErrorDomain Code=1 internalError state=3 before guest) — narrow layer is not IP_FORWARDING_FAILURE etc., those layers not reached. Do not reopen VZ boot diagnostics unless VM genuinely fails before guest (it does, at start). No guest logs to classify.

If VM recovers, potential next failures would be classified narrowly: IP_FORWARDING_FAILURE (cat /proc/sys/net/ipv4/ip_forward !=1), DOCKER_BRIDGE_FAILURE (no docker0), VETH_FAILURE (veth modprobe), GUEST_NAT_FAILURE (iptables MASQUERADE), CONTAINER_OUTBOUND_FAILURE (ping 1.1.1.1), CONTAINER_DNS_FAILURE (nslookup), DOCKER_PORT_PUBLISH_FAILURE (docker run -p), HOST_FORWARD_DISCOVERY_FAILURE (no HARPOON_GUEST_IP), HOST_PORT_FORWARD_FAILURE (curl EOF or connect refused).

## Remaining Manual Verification (when host recovers)

From normal Terminal (not Muse):

```bash
cd ~/Documents/Github/Harpoon && ./spike2/build/harpoon-spike2-vsock
# wait for HARPOON_DOCKER_READY and HOST_FORWARD_LISTENING in stderr and serial

export DOCKER_HOST=unix:///tmp/harpoon-docker.sock

# Phase 2
docker info 2>&1 | grep -E "Forward|Bridge"
cat /tmp/harpoon-spike2-serial.log | grep HARPOON_IP_FORWARD  # expect 1
cat /proc/sys/net/ipv4/ip_forward  # via docker run --rm alpine:3.22 cat /proc/sys/net/ipv4/ip_forward or via serial grep
# guest check: docker run --rm alpine cat /proc/sys/net/ipv4/ip_forward ==1

# Phase 3
docker run --rm alpine:3.22 ping -c 1 1.1.1.1
docker run --rm alpine:3.22 nslookup dl-cdn.alpinelinux.org  # expect Address
# also: docker run --rm alpine:3.22 wget -O- http://example.com (outbound)

# Phase 4-6
docker run --rm -d --name harpoon-web -p 8080:80 nginx:alpine
docker ps  # verify harpoon-web Up, Ports 0.0.0.0:8080->80
docker inspect harpoon-web 2>&1 | grep -A2 HostPort  # 8080
ip addr show docker0  # in serial HARPOON_BRIDGE_CHECK
curl -v http://127.0.0.1:8080/  # expect <html> nginx Welcome, not connection refused/EOF
# Host listener verify:
lsof -nP -iTCP:8080 -sTCP:LISTEN  # 127.0.0.1:8080 (not 0.0.0.0)
netstat -an | grep 8080

# API still vsock-only
DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker version  # Server linux/arm64
DOCKER_HOST=unix:///tmp/harpoon-docker.sock docker ps
```

Production port discovery later: subscribe to Docker events `docker events --filter type=container --filter event=start` or poll `docker ps --format '{{.Ports}}'` and `docker inspect -f '{{json .NetworkSettings.Ports}}'` to learn HostPort->ContainerPort mappings, then (re)configure host forwarders. Spike hardcodes 8080:80.

## Remaining Limitations

- Host framework transient persists (92% disk, VZ state) — needs natural clear or reboot, not code
- VZNAT IP connectivity host->guest via 192.168.64.x assumed usable per spec; if NAT blocks, fallback would be guest socat VSOCK-LISTEN:8081 -> 127.0.0.1:8080 and host vsock forward (not implemented, documented as alternative)
- iptables/nft hybrid: copies both legacy xt_* and nft modules; Docker may use nft on Alpine 3.22 — covered by copying full netfilter sets
- Single explicit forward 127.0.0.1:8080 only; no dynamic multi-port or LAN bind (0.0.0.0) — intentional loopback-only default
- No bind-mount/VirtioFS/volume/balloon/GUI/installer/Compose/K8s (scope boundary)
