# Distribution

See [Installation](installation.md) for usage and [Architecture — Distribution (M11)](architecture.md#distribution-m11) for layout.

Staged `dist/harpoon-0.1.0-dev-darwin-arm64` is relocatable (`bin/../lib/harpoon`), verified `cd /tmp && harpoon doctor` PASS. Archive `tar.gz` 289M + `sha256` `c2930f90…` (`shasum -c` OK). Signing ad-hoc (`-`) with `com.apple.security.virtualization` (`valid on disk`), not notarized — `spctl` expected fail until Developer ID + `notarytool`.

User disk `~/Library/.../data/harpoon-root.img` (fallback `/tmp/harpoon-runtime/data`) is provisioned via `cp -c` clone (2.0G/962M), preserved across reinstall/uninstall.

## A release is distributed only when its asset is verified

Notes that name a DMG and a checksum do not distribute anything: `v2.0.0-rc.2` was published with no assets. After
uploading the DMG that `tools/release/macos/release.sh` produced, run
`tools/release/macos/verify-release-assets.sh vX.Y.Z dist/vX.Y.Z/Harpoon-X.Y.Z-ARCH.dmg`. It fails unless the release
has an asset with that file name, uploaded, of the same size, with the same SHA-256. `release.sh` prints this as its
required next step. Before that, `tools/verify-bundle.sh` (run by the build, DMG and release scripts) checks the bundle
itself: the desktop reads schema-2 journals and names the schema it supports, the Swift runtime does not open the
journal, and no acceptance executable, `harpoon-deploy` tool or removal marker is inside.
