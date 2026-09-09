# 0003. Runtime Closure — Offline Guest and Release Gate

- **Status:** accepted
- **Date:** 2026-09-02

## Context

Stage-5 live acceptance on the work Mac failed: the shipped `511313c8` RC had no `resize2fs` in `initramfs` nor `harpoon-root.img`; `src/init` did `apk add e2fsprogs` after networking, so offline 16G/2G reconcile failed before Docker. Structural verifier only grepped source, not artifacts. Broader audit showed `python3` not in either artifact (fresh install would need network for `harpoon-mgmt`), while Docker/socat were already pinned in the root template.

Research principle: every dependency for boot→Docker→management must be classified A (macOS) / B (Harpoon.app) / C (initramfs) / D (root template), never E (network) for core runtime.

## Decision

We will embed `resize2fs 1.47.2-r2` + `libext2fs,libe2p,libcom_err,libblkid,libuuid,mke2fs.conf` in canonical `harpoon-initramfs.cpio.gz` and copy to final root before `switch_root` (`HARPOON_RESIZE2FS_REFRESH`). We will run `HARPOON_DISK_CHECK_START` **before** `HARPOON_APK_UPDATE_START` (offline), keeping a second idempotent check before `HARPOON_DOCKERD_START`. We will make `apk` offline-aware (`HARPOON_APK_SKIPPED` if `dockerd` already present, `e2fsprogs` no longer `apk add`ed, `python3` no longer `apk add`ed because it is pinned in the root template). We will pin `python3 3.12.14-r0` + `libpython3.12.so.1.0` + stdlib (`json`, `subprocess`, `pty`, `select`, `fcntl`, `termios`, etc., with `lib-dynload` extensions) and `harpoon-mgmt` itself in canonical `harpoon-root.img` (2G logical, ~322M physical), eliminating the last network dependency for core startup. We will add artifact-level `verify-runtime-closure.sh` (host arch/minos/dylibs/entitlements, initramfs BusyBox 24 applets + ELF loader/DT_NEEDED/aarch64 + modules verified via portable `llvm-readelf`/`readelf` with actual `PT_INTERP` path, root `dockerd/docker/containerd/socat` + `python3` hard gate + ELF closure + `harpoon-mgmt` imports, boot order offline with `python3` also offline) and `regression-m17-offline.sh` (2G→16G `resize2fs` harness → `17179869184` plus `python3`/`harpoon-mgmt`/`libpython`/`lib-dynload` structural proof and `apk` offline checks).

## Alternatives Considered

- Pre-install `e2fsprogs` only in `harpoon-root.img` at `mkfs.ext4` time: would require rebuilding 2G template with Docker+python3; deferred to next iteration for `python3` (root currently has Docker but not python3). Initramfs path is smaller and proves offline before any mount.
- Static `resize2fs`: Alpine `e2fsprogs-static` only ships `.a`, no binary — rejected.
- Keep `apk add` for `resize2fs`: rejected — violates offline self-containment.

## Consequences

Fresh `Harpoon.app` on clean macOS can now boot VM, offline reconcile 16G/2G → 16G, start Docker, and start `harpoon-mgmt` (vsock 2377) with **no network**; `release.sh` now refuses if closure fails (including `python3` hard gate). Network is only required for user image pulls (`docker pull`) or optional `apk` maintenance/upgrades — no core Harpoon startup dependency remains on `E` (network). Verified via `verify-runtime-closure.sh` (hard FAIL on missing `python3`/`harpoon-mgmt`/`libpython`/`PT_INTERP`), `verify-guest.sh`, `regression-m17-offline.sh` (structural proof for `resize2fs`/`Docker`/`python3`/`harpoon-mgmt` + ELF + `apk` offline), and `verify-bundle.sh`. Previous `WARN` python3 gap is now closed; repository is justified for one final RC transfer to the work-Mac.

