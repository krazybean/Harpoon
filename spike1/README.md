# Spike 1 — Virtualization Foundation

Prove Apple Virtualization.framework can boot ARM64 Linux under Harpoon's intended Rust-centric architecture.

## What it proves

- `VZVirtualMachine.isSupported == true` on this host
- `VZLinuxBootLoader` + `VZGenericPlatformConfiguration` can create/validate/start a VM (requires `com.apple.security.virtualization` entitlement)
- ARM64 Linux kernel boots with minimal initramfs
- Guest serial output (`hvc0`) reaches host via `VZFileHandleSerialPortAttachment` (deterministic boot detection)
- Clean shutdown via `requestStop` / `stop`
- Useful host-side logging and failure reporting

## Guest provenance

- **Source:** https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/
- **Files:** `vmlinuz-virt` (Alpine virt kernel 6.12.x, GPL-2.0) + `initramfs-virt` (Alpine) and minimal `harpoon-initramfs.cpio.gz` built locally
- **Version pin:** Alpine 3.22 netboot, fetched 2026-08-24; checksums recorded in `fetch_guest.sh` output
- **Reproducible:** `spike1/fetch_guest.sh` re-fetches same URLs; `spike1/cache/` is gitignored (no huge blobs committed)
- **License:** Alpine GPL-2.0, kernel.org compatible

## Repeatable invocation

```bash
spike1/fetch_guest.sh   # fetch kernel + initramfs (once), record sha256
spike1/run.sh           # build Swift runner, sign, boot VM, capture logs
# logs: spike1/build/vm.log (host), spike1/build/guest.out (guest serial)
```

Without network, spike fails fast with missing-file error (not hidden state).

## Architecture finding: Rust vs Swift bridge

- Swift `Virtualization.framework` usage is authoritative and minimal (one file `spike1/swift/main.swift`).
- Direct Rust `objc2`/`block2` bindings are possible (crate `objc2` 0.6.4) but block-callback lifetimes are fragile.
- **Spike 1 uses Swift runner** as the smallest credible proof; production may keep a *tiny Swift bridge* (one Swift file compiled with `swiftc`) called from Rust daemon via FFI/subprocess rather than contorting Rust. This is documented in `docs/decisions/0002-rust-swift-bridge.md`.

## Rust control proof (additional)

`spike1/rust/` contains minimal Rust validation (cargo project) attempting to call same APIs via `objc2`. See below.

## Expected result

- Exit 0, `vm.log` contains `BOOT_DETECTED` (saw `HARPOON_SPIKE_OK` or `Linux version`/`Welcome to Alpine`), then `SHUTDOWN_OK`.
- Failure modes printed with `VZErrorDomain` code/domain and suggestion (e.g., missing entitlement Code=2, missing kernel).

## Why this is not premature architecture

Spike is ugly where appropriate: no multiple VM types, no Kubernetes, no GUI. One VM, one kernel, one serial pipe.

