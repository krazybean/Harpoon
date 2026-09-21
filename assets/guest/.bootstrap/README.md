# Guest artifact bootstrap

Harpoon's source tree intentionally does not commit the production kernel (`Image-virt`) or sparse root image (`harpoon-root.img`). The current v0.1.1 GitHub release publishes the desktop DMG and `SHA256SUMS`, but it does **not** publish a standalone guest-artifact bundle for rebuilding a release from a fresh clone.

Place trusted production guest artifacts here when bootstrapping a source-built release without using historical prototype directories:

- `harpoon-initramfs.cpio.gz`
- `harpoon-root.img`

```sh
mkdir -p assets/guest/.bootstrap
# copy from a trusted local build (not spike1/spike2)
cp /path/to/trusted/harpoon-initramfs.cpio.gz assets/guest/.bootstrap/
cp /path/to/trusted/harpoon-root.img assets/guest/.bootstrap/
bash tools/guest-builder/build-initramfs.sh
bash tools/guest-builder/build-root.sh
```

These bootstrap files are ignored by `.gitignore` and are not release artifacts themselves. A future standalone guest-artifact release should publish versioned files plus SHA-256 provenance before this bootstrap path is retired.

Do NOT point this at spike1/ or spike2/.
