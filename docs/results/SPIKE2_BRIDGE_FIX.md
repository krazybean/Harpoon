# Spike 2 Bridge — half-close fix (host only)

Date: 2026-08-25T02:19:10Z rebuilt

## Host harness
- spike2/build/harpoon-spike2-vsock 156K e7e98607dfb0f39f3284696e370dbd6ee71ab1ff4dbcb0d6fd9e4ba7db4f4e1e
- entitlements com.apple.security.virtualization signed
- block device VZDiskImageStorageDeviceAttachment spike2/cache/harpoon-root.img 2G ext4 sparse raw preserved
- vsock VZVirtioSocketDevice port 2375 preserved

## Bridge fix — transparent byte-stream, half-close, keep-alive, concurrent

Previous proxy did `closeBoth()` on any read 0, closing opposite direction before response body fully forwarded → `docker info` saw `unexpected EOF` (Go http client expects full `Content-Length`/`chunked` body, got FIN).

New proxy per connection `id` (BRIDGE_ACCEPT <id>):
- BRIDGE_VSOCK_CONNECTED <id>
- BRIDGE_CLIENT_EOF <id> → cancel clientRead, shutdown(vsock, SHUT_WR), keep vsockRead for response
- BRIDGE_VSOCK_EOF <id> → cancel vsockRead, shutdown(client, SHUT_WR), keep clientRead for keep-alive next request
- BRIDGE_CLIENT_SHUT_WR / BRIDGE_VSOCK_SHUT_WR logged
- Only `BRIDGE_CLOSE <id> <reason>` when both directions EOF or error (EPIPE/EAGAIN/EINTR handled, partial writes retried)
- Handles HTTP keep-alive (multiple requests on one Unix socket → one vsock), concurrent clients (each id independent vsock), no Docker HTTP parsing.

No guest change: spike2/cache/harpoon-docker-initramfs.cpio.gz 64c845a... and harpoon-root.img unchanged.

## Remaining warning
`IPv4 forwarding is disabled` from `docker info` — host/guest `net.ipv4.ip_forward` next-stage, not a bridge failure. Documented for Spike 3 networking.

## Manual verification
export DOCKER_HOST=unix:///tmp/harpoon-docker.sock
docker version # Server 28.3.3 linux/arm64
docker info    # expect no unexpected EOF, full Server output
docker run --rm hello-world # expect Hello from Docker!

Previous live: hello-world already PASS via block-backed ext4 (pivot_root fixed), docker info EOF was only remaining defect.
