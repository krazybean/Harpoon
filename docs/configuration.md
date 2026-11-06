# Configuration

Harpoon uses a small JSON file for user defaults. CLI flags always win, and environment variables override the file for `cpus` and `memory`.

## Location

```
~/Library/Application Support/Harpoon/config.json
```

Fallback for sandbox: `/tmp/harpoon-runtime/config.json` (tests only). Production uses the Library path.

## Precedence

```
CLI flag > environment > user config > compiled default      (cpus, memory)
```

Compiled defaults: `cpus=2`, `memory=4096` MiB, `disk` `spike2/cache/harpoon-root.img`, etc. The environment (`HARPOON_CPUS`, `HARPOON_MEMORY_MIB`) overrides the config file and is overridden only by CLI flags. This is the order the runtime applies (`RuntimeConfig.fromEnvironment`, then `resolveConfigFromArgs`), and `harpoon config set` therefore has no effect on a run that sets one of those variables.

Other settings:

| Setting | Order |
|---|---|
| kernel, initramfs, disk path | CLI flag (`--kernel`, `--initramfs`, `--disk`) > environment (`HARPOON_KERNEL`, `HARPOON_INITRAMFS`, `HARPOON_DISK`) > built-in lookup. They are not config-file keys. |
| disk size (new disk only) | `--disk-size` > `HARPOON_DISK_SIZE` > config `disk-size` > 32G, the same order as `cpus` and `memory`. A value that is not a size or is below 2G is skipped and the next layer applies. An existing disk is never resized by any of them; use `harpoon disk resize`. |

## Commands

```
harpoon config show          # path + values
harpoon config path          # print path
harpoon config set cpus 2    # 1...8
harpoon config set memory 1024  # MiB, 512 or more
harpoon config reset cpus
harpoon config reset memory
harpoon config reset all
harpoon config --help
```

## Validation

- `cpus` 1...8; `memory` (MiB) from 512 up to the maximum Virtualization.framework allows on the host (the examples below use 768 and 1024; the compiled default is 4096). Invalid `harpoon config set memory 128` exits 1 and does not corrupt existing config.
- Malformed `config.json` → `Harpoon configuration is invalid: <reason> Config: <path>` with hint `harpoon config reset ...` or fix JSON. `harpoon doctor` also reports it. File is not silently discarded.

## Examples

```
harpoon config set memory 768
harpoon start          # → 768 MiB (from config)
harpoon restart        # → still 768
harpoon restart --memory 1024  # → 1024 this run, config still 768
harpoon config show    # → memory: 768
```

## Implementation

Atomic writes via `Data.write(..., .atomic)`. No general framework.

## Exit Codes

- `config set` invalid → 1
- `config show` malformed → 1
