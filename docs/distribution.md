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

## Signing an already-built DMG (no rebuild)

`tools/release/macos/sign-existing-dmg.sh INPUT.dmg` signs the app inside a DMG that CI already built and verified, so
the Rust, Swift and guest assets are not rebuilt and `tools/guest-builder` is not run. The DMG is a read-only compressed
image, so the script mounts it writable through a **shadow file** (`hdiutil attach -shadow`, copy-on-write: only the
blocks signing changes are stored), runs the existing `sign-app.sh` and `verify-signatures.sh` on the mounted app,
converts the image back with the same shadow, and proves that **only the four executables and `CodeResources` differ**
from the input. The input is never modified and the output is a separately named `*-signed.dmg` (never overwritten).

Stages, each only when asked for: the default signs the app and converts (it uses the Developer ID key; nothing goes to
Apple); `--dry-run` checks and plans only; `--sign-dmg` also signs the image; `--submit-to-apple` also notarizes
(`notarize.sh` waits and staples), validates, and writes `SHA256SUMS` and `SIGNING-PROVENANCE.txt`. The free space of the
work volume must stay above 16 GiB (absolute floor 15 GiB) before each major step. The **final** SHA-256 is that of the
signed, notarized, stapled file; the SHA-256 of the unsigned CI image is recorded only as the build input.

The regression (`regression-sign-existing-dmg.sh`) is its own CI check, **`signing-tooling`** (`.github/workflows/signing-tooling.yml`),
not part of `validate`: it needs `hdiutil`, runs under a 10-minute job limit with its own watchdogs, prints which scenario and
command is running, and on failure prints the disk-image state of the runner. Every disk-image command in the script runs under a
watchdog (`run_bounded`), so a hang fails with the command named. Image contents are compared **without following symlinks**:
an earlier `diff -r` followed the image's `Applications -> /Applications` link into the host's real applications, which is slow
and endless on a runner full of apps.

Measured on a synthetic image (`regression-sign-existing-dmg.sh`): a shadow holds only the changed blocks (3.2 MB for a
32 MB image whose code and seal changed), the conversion output is about the input's size, and `hdiutil` neither detects
a shadow from a different image nor reuses a half-finished one safely, so the script refuses an existing shadow and
never reuses one. `hdiutil convert -format` prints a deprecation notice (macOS points to `diskutil image create`); it works.
