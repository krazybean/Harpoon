# Distribution

See [Installation](installation.md) for usage and [Architecture — Distribution (M11)](architecture.md#distribution-m11) for layout.

Staged `dist/harpoon-0.1.0-dev-darwin-arm64` is relocatable (`bin/../lib/harpoon`), verified `cd /tmp && harpoon doctor` PASS. Archive `tar.gz` 289M + `sha256` `c2930f90…` (`shasum -c` OK). Signing ad-hoc (`-`) with `com.apple.security.virtualization` (`valid on disk`), not notarized — `spctl` expected fail until Developer ID + `notarytool`.

User disk `~/Library/.../data/harpoon-root.img` (fallback `/tmp/harpoon-runtime/data`) is provisioned via `cp -c` clone (2.0G/962M), preserved across reinstall/uninstall.
