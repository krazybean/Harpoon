# Installation & Distribution (M11)

Harpoon is a self-contained, relocatable macOS installation. No source tree is required after install.

## Artifacts

- `dist/harpoon-0.1.0-dev-darwin-arm64/` — staged layout
  - `bin/harpoon` (802K, arm64, ad-hoc signed `com.apple.security.virtualization`, `valid on disk`)
  - `lib/harpoon/Image-virt` (33M kernel)
  - `lib/harpoon/harpoon-initramfs.cpio.gz` (14M, sha `70a89d585bf0efbe...`)
  - `lib/harpoon/harpoon-root.img` (2.0G logical, 962M allocated APFS sparse, ext4)
  - `install.sh`, `uninstall.sh`, `share/doc/harpoon/`
- `dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz` (289M) + `.sha256` (`c2930f90a80f...` — regenerate and `shasum -a 256 -c`)

## Install

From a checked-out repo or extracted archive:

```sh
bash harpoon/install.sh          # installs to /usr/local (needs sudo for /usr/local)
# or from staged archive:
tar xzf dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz
./dist/harpoon-0.1.0-dev-darwin-arm64/install.sh
```

Install checks `harpoon status`/`lsof /tmp/harpoon.lock` and refuses if running (`harpoon stop` first). Binaries are `cp -c` (APFS clone) aware for the sparse root image; lib files are `644`, bin `755`.

## Run anywhere

```sh
cd /tmp
harpoon version   # 0.1.0-dev
harpoon doctor    # 11 PASS when staged/installed
harpoon start     # block device attached /tmp/harpoon-runtime/data/harpoon-root.img or ~/Library/...
harpoon status    # running
docker --context harpoon version
docker --context harpoon run --rm hello-world
harpoon stop
```

Resource resolution (`RuntimeConfig.resolveResource`/`resolveRootDisk`):

1. `installedLibDir` candidates: `/usr/local/lib/harpoon`, `/opt/homebrew/lib/harpoon`, `bin/../lib/harpoon` (relocatable), then cwd fallback `spike1/cache/...` for dev.
2. User disk precedence: `~/Library/Application Support/Harpoon/data/harpoon-root.img` then `/tmp/harpoon-runtime/data/harpoon-root.img` (sandbox fallback). First run provisions by `cp -c`/`ditto` clone from template; logical size verified (2G). Subsequent runs reuse existing user disk — reinstall does not overwrite.

## Uninstall / Upgrade

```sh
harpoon stop
bash harpoon/uninstall.sh          # removes /usr/local/bin/harpoon + /usr/local/lib/harpoon, preserves user data, removes harpoon docker context if owned
bash harpoon/uninstall.sh --purge  # also removes ~/Library/Application Support/Harpoon and /tmp/harpoon-runtime

# upgrade: stop, then install.sh again (same prefix) — user disk preserved
bash harpoon/install.sh
```

## Verification

```sh
codesign --verify --verbose /usr/local/bin/harpoon  # valid on disk
codesign -d --entitlements :- /usr/local/bin/harpoon | grep virtualization
spctl --assess --type execute --verbose /usr/local/bin/harpoon  # ad-hoc: internal error expected; Developer ID + notarization required for Gatekeeper PASS
shasum -a 256 -c dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz.sha256
bash harpoon/m11-test.sh
```

## Known limitations

- Ad-hoc signature only (`-`); not notarized — Gatekeeper `spctl` fails until Developer ID + `notarytool` is added (no updater/GUI).
- Host transient `VZErrorDomain Code=1` (`Internal Virtualization error`) intermittently blocks `harpoon start` on this macOS host; retry once, then `harpoon stop` + retry. Not a packaging bug (observed with both staged and repo bins, both disks).
- Sandbox on this dev host blocks `mkdir ~/Library/Application Support/Harpoon/data` (`Operation not permitted`); production fallback to `/tmp/harpoon-runtime/data` is functional, but production hosts allow the primary path.
- `harpoon-root.img` is sparse APFS; `du -m` should show ~962M, not 36M (fixed from `FileManager.copyItem` truncation via `cp -c`/`ditto`).
