# Bootstrap — TEMPORARY until v0.1.1 publishes versioned artifacts

Place production guest artifacts here to bootstrap a fresh clone **without** using historical prototype directories:

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

These files are **ignored** by `.gitignore` (`assets/guest/.bootstrap/` not tracked).
Once https://github.com/Harpoon/releases/download/v0.1.1/ publishes, FETCH MODE will verify SHA-256 and this bootstrap will be unnecessary.

Do NOT point this at spike1/ or spike2/.
