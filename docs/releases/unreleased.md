# Unreleased changes (after v2.0.0-rc.3)

Changes merged to `main` since [rc.3](v2.0.0-rc.3.md) that are not in any published release. They are not in the rc.3 DMG.

## Behavior changes

### Disk size for a new root disk follows the same order as cpus and memory (#184)

`CLI flag > environment > config file > built-in default`: `harpoon start --disk-size`, then `HARPOON_DISK_SIZE`, then the
`disk-size` key in `config.json`, then 32 GiB.

**If you relied on the config file winning:** until now `harpoon config set disk-size 16G` overrode `HARPOON_DISK_SIZE`.
It no longer does. If `HARPOON_DISK_SIZE` is set in your environment (a shell profile, a launch agent, a script), it now
decides the size of the next new disk, and the config value applies only when the variable is unset. To keep using the
config file, unset the variable.

- The size applies only when a disk is **created**. An existing disk is never resized, truncated or replaced by any of these;
  use `harpoon disk resize` to grow it.
- A value that is not a size, or is below 2 GiB, is skipped and the next layer applies (previously an unparseable
  environment value was also ignored, but one below 2 GiB produced a template-sized disk). `--disk-size` and
  `harpoon config set disk-size` still reject such values with an error.
- The default is unchanged (32 GiB, sparse).

### Inspecting the root disk no longer creates it (#183)

`harpoon status --json`, `doctor`, `diagnostics`, a rejected `start`, and any command run with `HARPOON_DISK` or `--disk` used
to create the default root disk as a side effect. The disk is now created only when a VM is about to start for the first
time, and never when an explicit disk path is given. `status --json` can therefore report a `diskPath` that does not exist yet
before the first start, and `doctor` shows it as a warning (not yet created) instead of a pass.

## Documentation

- The help text, usage line and configuration guide now state the real precedence for `cpus` and `memory`: CLI flag >
  environment > config file > default (#181, #182).
