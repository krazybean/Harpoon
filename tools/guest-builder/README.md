# Guest Builder — Canonical Production Guest Assets

This directory is the **authoritative production guest build location**.

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

## Provenance

- Kernel: `https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/vmlinuz-virt`
  (Alpine 3.22 virt, 6.12.x). The `vmlinuz-virt` PE+gz wrapper is decompressed at
  the inner gzip offset to produce the uncompressed `Image-virt` required by
  `Virtualization.framework`.
- initramfs: built from `alpine-minirootfs` + virtio modules injected from
  `initramfs-virt`/`modloop-virt` (see `build-initramfs.sh` — production behavior, cleaned up).
  Two-mode: FETCH (versioned artifact with SHA-256) or REBUILD (Docker Linux).
  Until v0.1.1 publishes, a temporary bootstrap at `assets/guest/.bootstrap/` may be used.
- root: a 2G raw ext4 (`2147483648` logical) populated from pinned Alpine 3.22
  repositories by `src/alpine-root-builder-init` inside a disposable
  Virtualization.framework VM. The VM installs the declared Docker Engine,
  containerd, runc, Python, transport, networking, filesystem, and certificate
  closure with real `apk`, writes a package manifest, removes APK/Docker build
  residue, and exits. Docker Desktop/Podman are not part of this path.

## Bug fix

Historical prototype fetch had a relative `CACHE=".../cache"` that broke when
invoked outside repo root. This builder uses `SCRIPT_DIR`/`REPO_ROOT` absolute resolution — no relative CACHE bug.

## Fresh-clone bootstrap (temporary, until v0.1.1 publishes)

`assets/guest/` is ignored (multi-GB). On a fresh clone without published release artifacts,
`fetch-kernel.sh` succeeds (Alpine CDN), but `build-initramfs.sh` and `build-root.sh` will
fail clearly and instruct to place a bootstrap file at:

```
assets/guest/.bootstrap/harpoon-initramfs.cpio.gz
assets/guest/.bootstrap/harpoon-root.img
```

This bootstrap is **temporary** and will be removed once `https://github.com/Harpoon/releases/download/v0.1.1/` publishes versioned artifacts with SHA-256 verification (see `HARPOON_INITRAMFS_URL` / `HARPOON_ROOT_URL` in scripts).

Do not use historical prototype directories as bootstrap source.
