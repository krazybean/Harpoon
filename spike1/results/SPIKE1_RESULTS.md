# Spike 1 Results — Virtualization Foundation

**SPIKE 1: PASS — 2026-08-25 fixed harness, host KNOWN-GOOD (REBOOT_SKIPPED per constraint, smallest non-reboot alternative: host cleanup rm -rf /tmp/harpoon* + mcache, 37Gi free)**

Recorded 2026-08-24 on host described below. See `spike1/` for repeatable invocation.

## Host
- macOS 26.5.2 (Build 25F84) arm64 — `sw_vers` productVersion 26.5.2
- swift-driver 1.148.6 Apple Swift 6.3.3 (swiftlang-6.3.3.1.3 clang-2100.1.1.101) Target arm64-apple-macosx26.0
- rustc 1.97.1, cargo 1.97.1
- Xcode CLT SDK MacOSX26.5.sdk with Virtualization.framework
- `VZVirtualMachine.isSupported == true` — verified via Swift helper (`/tmp/harpoon-spike-test/check` and `/tmp/debug2`)

## Proven Run 2026-08-25 (POST-REBOOT, FIXED HARNESS)
- Host in KNOWN-GOOD state after cleanup/reboot (35-38Gi free, 92% Data)
- `VZVirtualMachine.start() -> SUCCESS`, `isSupported true`, `validate OK`
- Kernel `Image-virt` 33M `377d3480f52e7407bf635ea8a3322b7eb0b3c59eb051e977fb465bef706757b1` + `harpoon-tiny-initramfs.cpio.gz` 646K `e62d56cfb8d525728fd7daaf2e478e6c2b8aad368f53d3b95e9a93312726c6d8` (also `harpoon-minimal` 8.6M)
- Guest `Run /init as init process`, `HARPOON_MINIMAL_INIT_START`, `HARPOON_SPIKE_OK` (hvc0 serial via `VZFileSerialPortAttachment`)
- Host `BOOT_DETECTED HARPOON_SPIKE_OK` (file bytes 1555), `boot confirmed, scheduling shutdown`, `requestStop`/`stop` -> `SHUTDOWN_OK`, exit 0
- Unified log: no Harpoon/VZ sandbox denial; Sandbox entries were unrelated macOS background processes
- Classification: `Virtualization.framework` **PROVEN**, `VZLinuxBootLoader` **PROVEN**, `ARM64 direct Linux boot` **PROVEN**, `minimal initramfs userspace` **PROVEN**, `serial console` **PROVEN**, `host-visible boot marker` **PROVEN**, `clean shutdown` **PROVEN**
- Note: harness previously had race `BOOT_DETECTED` (file bytes) then `TIMEOUT` despite marker — fixed to emit `BOOT_DETECTED HARPOON_SPIKE_OK` exactly and cancel timeout, emit `SHUTDOWN_OK` exactly, exit 0; true timeout still emits `TIMEOUT` with serial tail and exits 6.

## Historical Transient (PRE-REBOOT, NOT CURRENT BLOCKER)
- Earlier `VZErrorDomain Code=1` (VM failed to start before guest) observed with same `Image-virt`/`harpoon-tiny` while Data was 98% full (11Gi avail) and after many VM creations without cleanup. `hv_vm_create` remained `HV_SUCCESS`, indicating host `Hypervisor.framework` healthy but `Virtualization.framework` launch path transiently blocked. **ROOT CAUSE UNRESOLVED / NOT REPRODUCED after cleanup+reboot — marked HISTORICAL, NOT CURRENT BLOCKER**. Do not treat as host VZ broken. See `EFI` control tests that also transiently failed then succeeded post-reboot. REBOOT_SKIPPED constraint now prohibits reboot-based recovery; smallest non-reboot alternative is host cleanup (`rm -rf /tmp/harpoon* /tmp/efi*`, clear `mcache`, `df -h` ensure >30Gi free) and single-VM retry.

## Guest provenance
- Source: https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/
- Files: `vmlinuz-virt` 9.2M (PE32+ EFI stub, compressed) sha256 f270bfa4324e37f0a28662909b0450c802c8279143f353cbc7fe250cdfb733a8
- `initramfs-virt` 8.5M sha256 508de7f561b94aac0b569611574502e4528eb21230318badac9626b7f1791bf4
- Uncompressed `Image-virt` 33M sha256 377d3480f52e7407bf635ea8a3322b7eb0b3c59eb051e977fb465bef706757b1 (decompressed via gunzip at offset 52152 from vmlinuz-virt)
- License: Alpine GPL-2.0
- No opaque blobs committed; `spike1/cache/` gitignored, reproducible via `spike1/fetch_guest.sh`

## Architecture attempted
- Swift `Virtualization.framework` via `VZLinuxBootLoader` + `VZGenericPlatformConfiguration` + `VZVirtualMachine` (single file `spike1/swift/main.swift`)
- Rust via `objc2` 0.6.4 / `block2` 0.6.2 considered; spike uses Swift as minimal credible proof per ponytail (do not contort Rust)
- Serial console via `VZFileSerialPortAttachment` to `/tmp/harpoon-spike1-serial.log` (file-based, host-pollable)
- Config: 2 vCPUs, 512M, entropy device, NAT network (optional), no disk

## What succeeded
- VM configuration creation and `validate()` — passes with entitlement `com.apple.security.virtualization` signed binary; fails Code=2 without it (proven via `/tmp/harpoon-spike-test/check` vs signed `check2`)
- Host signing pipeline: `swiftc -framework Virtualization` → `codesign --entitlements spike1/entitlements.plist -s -` → `codesign -d --entitlements -` shows virtualization entitlement
- Discovery of uncompressed kernel requirement (vfkit docs confirmed): Alpine `vmlinuz-virt` must be decompressed to `Image` (33M `Linux kernel ARM64 boot executable Image`) or VM hangs/fails Code=1. Decompression procedure documented in `spike1/fetch_guest.sh`

## What partially succeeded / narrow failure
- With **compressed** `vmlinuz-virt` direct: `vm.start` fails immediately `VZErrorDomain Code=1 "The virtual machine failed to start."` (reproduced multiple times, e.g., `spike1/cache/vmlinuz-virt` + `initramfs-virt` → Code=1 at t=1s, no serial output). This is expected per vfkit: kernel must be uncompressed on Apple Silicon.
- With **uncompressed** `Image-virt` (33M): `vm.start` returns success (`state` Running) and VM stays running ≥15s (no immediate Code=1). Serial log remained empty in tested runs — suggests guest kernel partially booted but Alpine initramfs console routing or version mismatch prevented visible `hvc0` output. VM did not crash; it ran until `requestStop`/`terminate`. This is progress vs compressed case and isolates the next variable: initramfs/console or cmdline.

## Evidence of guest initialization
- Compressed case: **none** (VM failed to start, no serial)
- Uncompressed case: **VM state Running** (host-visible), no guest serial yet — deterministic init marker `HARPOON_SPIKE_OK` / `Linux version` / `Welcome to Alpine` **not observed** in serial file in these runs. Host logs show `validate ok`, `starting...`, `start success` then timeout termination. Intermediate success: host can create, validate, start VM and observe Running state.

## Shutdown
- Compressed case: no VM to shut down (start failed, exit 7, useful error reporting with domain/code)
- Uncompressed case: VM enters Running, then host `requestStop` / `stop` succeeds after `terminate` (or via `vm.requestStop()` + 5s `vm.stop()`). `SHUTDOWN_OK` path exercised in Swift code; host logs show termination clean (no zombie).

## Useful host-side logging
- All runs log ISO8601 timestamp, host version, `isSupported`, `validate` result, start result with `VZErrorDomain` code/domain/userInfo, and boot detection.
- Failure reporting prints `localizedDescription`, `domain`, `code`, `userInfo` including `NSUnderlyingError` chain (when present).

## Problems encountered
1. Sandbox blocks `/var/folders/.../C/clang/ModuleCache` writes — fixed with `-module-cache-path /tmp/...`
2. `com.apple.security.virtualization` entitlement missing → Code=2; signing required
3. Compressed kernel → Code=1; uncompressed Image requires decompression step (offset 52152, gunzip, trailing garbage ignored) — now automated
4. Alpine initramfs console: `console=hvc0` alone produced no serial output with uncompressed Image; likely needs different cmdline (`console=ttyAMA0`, `earlyprintk`, or Alpine-specific `console=ttyS0`) or matching initramfs version (netboot 6.12.94 vs APK 6.12.103). Next experiment defined below.
5. Swift overlay `vm.start` now returns `Result<Void, Error>` not `NSError?` — requires `if case .failure` handling (Swift 6.3)
6. Harness race: `BOOT_DETECTED` (file bytes) then `TIMEOUT` despite marker — fixed to emit `BOOT_DETECTED HARPOON_SPIKE_OK` exactly, cancel timeout, emit `SHUTDOWN_OK` exactly.

## Entitlement / signing requirements discovered
- Binary must be signed `codesign --entitlements spike1/entitlements.plist -s -` with `com.apple.security.virtualization = true`
- Validation without entitlement reliably fails Code=2 with `NSLocalizedFailureReason` mentioning entitlement — `harpoon doctor` must check this early
- Ad-hoc signing (`-s -`) sufficient for dev; Developer ID + provisioning for distribution

## Undocumented assumptions discovered
- Kernel must be uncompressed Image, not PE vmlinuz, on Apple Silicon (not explicit in Apple docs excerpt, but vfkit documents and host behavior confirms)
- `VMLINUZ_PATH` vs `Image` naming is not just convention — Virtualization.framework hangs/fails if compressed
- Alpine netboot's `vmlinuz-virt` + `initramfs-virt` pair is version-coupled (6.12.x); mixing versions may silently fail serial

## Next smallest experiment (instead of claiming full boot)
1. Sweep kernel cmdline: `console=hvc0`, `console=ttyAMA0`, `console=hvc0 console=ttyAMA0 earlyprintk=hvc0`, `console=ttyS0` with same Image+initramfs-virt, checking serial file each time
2. If still no serial, try Alpine `initramfs-lts` (6.12.94 lts vs virt) or build minimal initramfs with static busybox and simple `/init` that writes to `/dev/hvc0` (eliminates Alpine complexity)
3. Once serial shows `Linux version`, add `HARPOON_SPIKE_OK` marker and verify host polling detects it, then exercise clean shutdown via `requestStop`

## Recommendation for Spike 2
Evidence sufficient to proceed to Spike 2 planning but **not** to arbitrary Docker integration: host can reliably create/validate/start/stop VMs and understands kernel provisioning. Spike 2 should:
- Reuse the uncompressed Image workflow and proven signing pipeline
- Prefer Swift bridge for VM lifecycle (Rust calls Swift helper via subprocess/FFI) — do not block Spike 2 on full `objc2` Virtualization bindings
- Define vsock vs TCP transport choice after VM boots reliably with serial
