# Configuration

Harpoon uses a small JSON file for user defaults. CLI flags always win.

## Location

```
~/Library/Application Support/Harpoon/config.json
```

Fallback for sandbox: `/tmp/harpoon-runtime/config.json` (tests only). Production uses the Library path.

## Precedence

```
CLI flag > user config > environment > compiled default
```

Compiled defaults: `cpus=2`, `memory=1024` MiB, `disk` `spike2/cache/harpoon-root.img`, etc. Environment (`HARPOON_CPUS`, `HARPOON_MEMORY_MIB`) overrides defaults but is overridden by config and CLI.

## Commands

```
harpoon config show          # path + values
harpoon config path          # print path
harpoon config set cpus 2    # 1...8
harpoon config set memory 1024  # 512|768|1024
harpoon config reset cpus
harpoon config reset memory
harpoon config reset all
harpoon config --help
```

## Validation

- `cpus` 1...8, `memory` 512|768|1024. Invalid `harpoon config set memory 128` exits 1 and does not corrupt existing config.
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
