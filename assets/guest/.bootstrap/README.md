# Guest artifact bootstrap

Harpoon's source tree intentionally does not commit the production kernel (`Image-virt`) or sparse root image (`harpoon-root.img`). The v0.1.1 GitHub release is still the latest stable **desktop** release and publishes a DMG and `SHA256SUMS`, but not a standalone guest-artifact bundle. **The rc.4 testing prerelease has a separate published, architecture-specific guest-assets bundle:** [guest-assets-3c36f31f8515](https://github.com/krazybean/Harpoon/releases/tag/guest-assets-3c36f31f8515), `harpoon-guest-assets.tar.gz` SHA-256 `3c36f31f85155f1b7075b002d2c5defa874f7bd9df5892517f8e9dd7b36ea32a`. Match the assets to the intended release and architecture.

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

These bootstrap files are ignored by `.gitignore` and are not release artifacts themselves. The published rc.4 guest-asset release has an explicit SHA-256. This local bootstrap path remains an optional fallback only; do not assume the scripts automatically select rc.4 guest assets for every historical version or build.

Do NOT point this at spike1/ or spike2/.
