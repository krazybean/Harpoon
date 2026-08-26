# M3 Docker Compatibility — Surface Inspected 2026-08-25

## Current Production Surface

- Binary `harpoon/build/harpoon` 327K signed `com.apple.security.virtualization` Swift 6.3.3 Virtualization.framework 1112.1.16
- Config: `RuntimeConfig` 2 vCPU 1024 MiB default 768/512 allowed, kernel `spike1/cache/Image-virt` 6.12.94-0-virt, initramfs `spike2/cache/harpoon-docker-initramfs.cpio.gz` dockerd 28.3.3, disk `spike2/cache/harpoon-root.img` 2G ext4, share `/tmp/harpoon-share` tag `harpoon-share`, socks `/tmp/harpoon-docker.sock` 0600 + `/tmp/harpoon-control` 0600, vsock 2375, forward 127.0.0.1:8080
- Lifecycle `STOPPED->STARTING->BOOTING->DOCKER_READY->RUNNING->STOPPING->STOPPED` + `FAILED`, `HARPOON_STATE` grep-friendly, `vm.start` != RUNNING gate `HARPOON_DOCKER_READY`
- Bridge `Bridges.swift` protocol-transparent: host Unix 0600 -> `VZVirtioSocketDevice.connect(toPort:2375)` -> guest `socat VSOCK-LISTEN:2375 -> /var/run/docker.sock`, per-client `DispatchSourceRead` full-duplex byte proxy, half-close `shutdown(SHUT_WR)`, keep-alive (multiple HTTP requests on one vsock fd), concurrent (each client independent), no HTTP parsing, 8192 byte loops handle partial writes/EAGAIN/EINTR/EPIPE, balloon control per-client buffered newline/EOF

## Docker Versions

Host CLI `Docker 29.3.1 API 1.54` (amd64 rosetta) observed `Client: 29.3.1 Go1.26.1`.
Guest daemon `Docker Engine 28.3.3 API 1.51` (from `spike2/cache` Alpine 3.22, verified via prior `docker version` `Server: 28.3.3`). API negotiation: client auto-downgrades to 1.51 — observed working in spikes, bridge transparent.

## Host Socket Contract

`/tmp/harpoon-docker.sock` mode 0600 `srw-------` owned 501. `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` works via vsock bridge only after `DOCKER_READY` + `HARPOON_RUNNING`. `docker version/info` succeed when VM RUNNING.

## Existing Tests

No repo-level tests for Docker compatibility; spike evidence in `docs/results/SPIKE*` and `spike2/TRANSPORT.md` (vsock preferred). M1/M2 acceptance via `harpoon/README.md`.

## Known Gaps Before M3

- Daemon label `harpoon.runtime=true` not yet set (requires guest initramfs rebuild, deferred per spec if invasive).
- BuildKit path not yet recorded (Docker 28.3.3 includes `docker build` without Desktop, but host `docker buildx` plugin expects `unix:///tmp/harpoon-docker.sock` — needs live verification).
- Large response streaming not yet bounded-tested beyond spike hello-world.
