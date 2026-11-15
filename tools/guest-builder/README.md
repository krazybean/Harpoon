# Guest Builder — Canonical Production Guest Assets

This directory is the **authoritative production guest build location**.

**Published reference (2026-11-15):** [v2.0.0-rc.4](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.4) packages verified arm64 and x86_64 guest assets. The corresponding **[guest-assets-3c36f31f8515](https://github.com/krazybean/Harpoon/releases/tag/guest-assets-3c36f31f8515)** release contains `harpoon-guest-assets.tar.gz` (SHA-256 `3c36f31f85155f1b7075b002d2c5defa874f7bd9df5892517f8e9dd7b36ea32a`). v0.1.1 is the latest stable **desktop** release; guest-asset bundle versions are separate from desktop version tags.

It produces the three canonical assets consumed by production runtime,
`harpoon/package.sh`, and Tauri bundling:

```
assets/guest/
  Image-virt                  # ARM64 Linux kernel (uncompressed, ~33M)
  harpoon-initramfs.cpio.gz   # immutable boot/runtime closure (~14M)
  harpoon-root.img            # 2G sparse Alpine/Docker/Python ext4 template
```

Historical prototypes remain frozen evidence and MUST NOT be referenced by production code.

## Quick start

> **Known verification drift (rc.4):** `verify-guest.sh` still searches `RuntimeConfig.swift` for the removed **32 GiB** default. Current new-disk default is **4 GiB**, so that check reports a false failure for both architectures even when the manifest and all other guest checks pass. This is tracked in the [rc.4 known limits](../../docs/releases/v2.0.0-rc.4.md#known-limits). The full `build.sh` gate should not be reported green until the stale assertion is updated; do not ignore unrelated failures.

```sh
# build all three assets (kernel + initramfs + root)
bash tools/guest-builder/build.sh

# individual steps
bash tools/guest-builder/fetch-kernel.sh
bash tools/guest-builder/build-initramfs.sh
bash tools/guest-builder/build-root.sh --builder vz --force
bash tools/guest-builder/verify-root.sh       # fails if dirty (containers/images/volumes != 0)
bash tools/guest-builder/sanitize-root.sh     # deterministic clean to 0/0/0

# after changing the kernel or initramfs outside build.sh, re-pin the pair;
# the runtime refuses to boot a pair that does not match the manifest
bash tools/guest-builder/guest-assets-manifest.sh --write
```

## x86_64 guest (Intel Macs, universal builds)

Every script honors `HARPOON_GUEST_ARCH` (`aarch64` default, or `x86_64`); the
per-arch pins live in `guest-arch.sh`. x86_64 assets go to `assets/guest/x86_64/`,
which the runtime uses on Intel hosts and universal app bundles carry in
`lib/harpoon/x86_64/`.

```sh
# on an Apple Silicon Mac with Rosetta and Docker; aarch64 assets must exist first
HARPOON_GUEST_ARCH=x86_64 bash tools/guest-builder/build.sh
```

- Kernel: the x86_64 `vmlinuz-virt` bzImage is used as shipped.
- root: the pinned aarch64 builder VM populates the x86_64 root with `apk --arch x86_64`
  and runs package scripts under Rosetta (`VZLinuxRosettaDirectoryShare` + binfmt_misc).
- initramfs: rebuilt in an `alpine:3.22` container with `--platform linux/amd64`.

GitHub's macOS runners cannot run Virtualization.framework, so CI's universal DMG
uses guests built on a Mac: `tools/release/macos/publish-guest-assets.sh` builds both,
uploads them as a pre-release asset, sets the workflow's repository variables and
re-runs the Universal macOS DMG workflow.

## Provenance

- Kernel: `https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot-3.22.6/vmlinuz-virt`
  (Alpine 3.22.6 virt, 6.12.110-0-virt). The `vmlinuz-virt` PE+gz wrapper is decompressed at
  the inner gzip offset to produce the uncompressed `Image-virt` required by
  `Virtualization.framework`.
- initramfs: built from `alpine-minirootfs` + virtio modules injected from
  `initramfs-virt`/`modloop-virt` (see `build-initramfs.sh` — production behavior, cleaned up).
  Two-mode: FETCH (versioned artifact with SHA-256) or REBUILD (Docker Linux).
  A trusted local bootstrap at `assets/guest/.bootstrap/` remains an optional fallback for source builds; the older "until v0.1.1 publishes" guidance is obsolete.
- root: a 2G raw ext4 (`2147483648` logical) populated from pinned Alpine 3.22
  repositories by `src/alpine-root-builder-init` inside a disposable
  Virtualization.framework VM. The VM installs the declared Docker Engine,
  containerd, runc, Python, transport, networking, filesystem, and certificate
  closure with real `apk`, writes a package manifest, removes APK/Docker build
  residue, and exits. Docker Desktop/Podman are not part of this path.

## Bug fix

Historical prototype fetch had a relative `CACHE=".../cache"` that broke when
invoked outside repo root. This builder uses `SCRIPT_DIR`/`REPO_ROOT` absolute resolution — no relative CACHE bug.

## Fresh-clone bootstrap (fallback for unavailable pinned assets)

`assets/guest/` is ignored (multi-GB). **Published rc.4 guest artifacts now exist**; see the versioned release above and verify its SHA-256 before using it. On a fresh clone without the pinned assets available locally,
`fetch-kernel.sh` succeeds (Alpine CDN), but `build-initramfs.sh` and `build-root.sh` will
fail clearly and instruct to place a bootstrap file at:

```
assets/guest/.bootstrap/harpoon-initramfs.cpio.gz
assets/guest/.bootstrap/harpoon-root.img
```

This bootstrap is a **fallback**, not a new release artifact or a replacement for checking the exact architecture, manifest and package checksum. Do not assume a published guest-assets release is automatically selected by every local build path; consult the pinned script inputs and `HARPOON_INITRAMFS_URL` / `HARPOON_ROOT_URL` overrides before building.

Do not use historical prototype directories as bootstrap source.
