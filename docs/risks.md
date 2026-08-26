# Risks

## Virtualization Framework

- **VZ startup reliability — UNRESOLVED HOST/FRAMEWORK STATE ISSUE (2026-08-25T02:00:58Z):** `VZVirtualMachine.start()` returns `VZErrorDomain Code=1` `Internal Virtualization error` *before* guest, `isSupported=true`, `validate OK`, `hv_vm_create HV_SUCCESS` historically. Spike 1 architectural proof remains **PROVEN** (2026-08-25T00:51:54 `Running` `HARPOON_SPIKE_OK` `BOOT_DETECTED` `SHUTDOWN_OK` on `Image-virt` `377d3480...`). Current host immediately reproduces `CYCLE_1_START_FAIL Code=1 state=3` on fresh `VZVirtualMachineConfiguration`/`VZVirtualMachine` with fresh serial (`spike1/build/harpoon-diag` 3-cycle diagnostic, no reuse). Classification per diagnostic: `HOST CURRENTLY IN TRANSIENT VZ START FAILURE` — `REBOOT_SKIPPED` per hard constraint, no reboot, no host diagnostic matrix repeat unless new evidence. **Impact:** can block VM execution transiently; **Spike 1 proof: unaffected** (historical), **Spike 2 implementation: unaffected** (hardened offline, execution-ready). **Historical Code=1 when Data 98% full is superseded** — current 37Gi free still Code=1, so disk-pressure hypothesis contradicted. No root cause claimed.

- **Entitlement gate:** `com.apple.security.virtualization` missing → `VZErrorDomain Code=2`. Signing must be part of build. Risk: dev builds without signing silently fail validation. Mitigation: `doctor` checks entitlement early.
- **API churn:** Apple may deprecate Linux bootloader options; pin macOS SDK and test each OS release.
- **Nested virtualization:** `genericPlatform.nestedVirtualizationSupported` false on some hardware — not required for MVP but document.

## Rust ↔ Apple boundary

- `objc2`/`block2` callback lifetimes are subtle; use-after-free → crash. Minimal Swift bridge may be safer. Spike 1 comparison mitigates.
- No stable `arcbox-vz` crate — avoid depending on third-party Virtualization wrappers; call framework directly or thin wrapper.

## Guest Supply Chain

- Kernel + rootfs provenance must be documented (Alpine/upstream), pinned version + sha256, reproducible curl. No opaque binaries. Disk image growth unbounded → monitor `~/.harpoon/vm/disk.img`.

## Docker Socket Security

- Host socket at `~/.harpoon/docker.sock` mode `0600`, directory `0700`; guest socket never world-readable. No TCP listen on 0.0.0.0 by default. Vsock isolates vs host TCP. `doctor` warns on stale socket.

## Persistence & Crash

- VM crash / host reboot / sleep-wake: guest disk must fsync; use `VZDiskSynchronizationModeFull` or `DataOnly` deliberately (measure). Daemon restart must reap stale sockets, not leave zombie VM process.

## Memory Reclamation

- Balloon inflation may not reduce `phys_footprint` measurably on macOS 26; may require specific guest balloon driver + config. Spike 5 must measure, not assume. `drop_caches` default harmful; keep EXPERIMENTAL.

## Networking

- VPNs (Cisco, Tailscale) may break VZNAT; need fallback documentation. Port forwards must handle dynamic `localhost` allocation conflicts.

## Distribution

- Code signing / notarization required for non-ad-hoc distribution; `com.apple.security.virtualization` entitlement needs provisioning profile. Validate `codesign -d --entitlements`.

