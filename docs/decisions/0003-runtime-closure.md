# 0003. Runtime Closure — Offline Guest and Release Gate

- **Status:** accepted
- **Date:** 2026-09-02

## Context

Stage-5 live acceptance on the work Mac failed: the shipped `511313c8` RC had no `resize2fs` in `initramfs` nor `harpoon-root.img`; `src/init` did `apk add e2fsprogs` after networking, so offline 16G/2G reconcile failed before Docker. Structural verifier only grepped source, not artifacts. Broader audit showed `python3` not in either artifact (fresh install would need network for `harpoon-mgmt`), while Docker/socat were already pinned in the root template.

Research principle: every dependency for boot→Docker→management must be classified A (macOS) / B (Harpoon.app) / C (initramfs) / D (root template), never E (network) for core runtime.

## Decision

We will embed `resize2fs 1.47.2-r2` + `libext2fs,libe2p,libcom_err,libblkid,libuuid,mke2fs.conf` in canonical `harpoon-initramfs.cpio.gz` and copy to final root before `switch_root` (`HARPOON_RESIZE2FS_REFRESH`). We will run `HARPOON_DISK_CHECK_START` **before** `HARPOON_APK_UPDATE_START` (offline), keeping a second idempotent check before `HARPOON_DOCKERD_START`. We will make `apk` offline-aware (`HARPOON_APK_SKIPPED` if `dockerd` already present, `e2fsprogs` no longer `apk add`ed). We will add artifact-level `verify-runtime-closure.sh` (host arch/minos/dylibs/entitlements, initramfs BusyBox 24 applets + ELF loader/DT_NEEDED/aarch64 + modules, root `dockerd/docker/containerd/socat`, boot order offline) and `regression-m17-offline.sh` (2G→16G `resize2fs` harness → `17179869184`).

## Alternatives Considered

- Pre-install `e2fsprogs` only in `harpoon-root.img` at `mkfs.ext4` time: would require rebuilding 2G template with Docker+python3; deferred to next iteration for `python3` (root currently has Docker but not python3). Initramfs path is smaller and proves offline before any mount.
- Static `resize2fs`: Alpine `e2fsprogs-static` only ships `.a`, no binary — rejected.
- Keep `apk add` for `resize2fs`: rejected — violates offline self-containment.

## Consequences

Fresh `Harpoon.app` on clean macOS can now boot VM and offline reconcile 16G/2G → 16G without network. `release.sh` now refuses if closure fails. `python3` fresh-install gap remains WARN (not blocking work-Mac 16G fixture which already has `harpoon-mgmt` deps after first boot where network was available); next RC should pin `python3` into root template to close fully.

