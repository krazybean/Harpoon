# Guest Builder — Canonical Production Guest Assets

This directory is the **authoritative production guest build location**.

It produces the three canonical assets consumed by production runtime,
`harpoon/package.sh`, and Tauri bundling:

```
assets/guest/
  Image-virt                  # ARM64 Linux kernel (uncompressed, ~33M)
  harpoon-initramfs.cpio.gz   # initramfs that apk-adds Docker at boot (~14M)
  harpoon-root.img            # 2G sparse ext4 template (962M physical, 0 containers/images/volumes)
```

Historical prototypes remain frozen evidence and MUST NOT be referenced by production code.

## Quick start

```sh
# build all three assets (kernel + initramfs + root)
bash tools/guest-builder/build.sh

# individual steps
bash tools/guest-builder/fetch-kernel.sh
bash tools/guest-builder/build-initramfs.sh
bash tools/guest-builder/build-root.sh        # creates sparse 2G ext4 if missing
bash tools/guest-builder/verify-root.sh       # fails if dirty (containers/images/volumes != 0)
bash tools/guest-builder/sanitize-root.sh     # deterministic clean to 0/0/0
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
- root: 2G raw ext4 (`2147483648` logical, ~962M physical APFS sparse). Created
  via `qemu-img`/`truncate` + `mkfs.ext4` when tools available, otherwise
  sanitized from existing template. Guest-template sanitation is mandatory before
  release (see `verify-root.sh` / `sanitize-root.sh`).

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
