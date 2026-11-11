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
| disk size (new disk only) | `--disk-size` > `HARPOON_DISK_SIZE` > config `disk-size` > 4G (provisional default; was 32G), the same order as `cpus` and `memory`. A value that is not a size or is below 2G is skipped and the next layer applies. An existing disk is never resized by any of them; use `harpoon disk resize`. |

## Disk capacity and expansion

A new root disk is **4 GiB logical** (sparse; provisional). An existing disk keeps whatever size it has, including 32 GiB ones, and an
explicit `--disk-size`, `HARPOON_DISK_SIZE` or `disk-size` still decides the size of a new one.

Harpoon watches the guest filesystem while it runs and warns **before** it fills. The runtime decides the level from `df`:

| Level | Condition (either) |
|---|---|
| warning | 80% used, or 1 GiB or less available |
| critical | 92% used, or 384 MiB or less available |

The absolute limits make small disks warn in time; the percentages cover large ones. ext4 keeps 5% for root, and `dockerd` runs as root,
so waiting for zero available leaves nothing to act with.

Growing the disk **stops the VM**, so it is always the user's decision:

- **Desktop:** a notice shows capacity, used/available space, host free space and suggested sizes (the next sizes above the current one that
  fit), with a custom size. *Not now* is remembered and does not repeat at the same level; it returns if usage climbs 5 more points or the
  level becomes critical (a dismissed critical notice repeats after an hour). *Confirm expansion* asks once more, says containers are
  interrupted, and only then stops Harpoon, grows the disk, starts it and checks that the filesystem grew and Docker is ready.
- **Terminal:** `harpoon disk status` prints the warning and the exact command; `harpoon disk status --json` is the same data for tools.
  `harpoon disk expand 8G --yes` performs the expansion; without `--yes` it prints what would happen and changes nothing. It never prompts,
  so scripts cannot hang, and nothing resizes or restarts without `--yes`. `harpoon disk resize` still grows a stopped Harpoon's disk.

Host space. The disk is a sparse file, so growing it allocates almost nothing at once. Harpoon distinguishes three quantities:

| | Meaning | Rule |
|---|---|---|
| immediate | ext4 metadata for the new space, about 2% of the added capacity | must fit in the host's free space **together with the reserve** (refused otherwise) |
| worst case | what the disk could take if it were filled: new size minus what is already allocated | allowed up to 4 times the headroom (free space minus the reserve); beyond the headroom the size is **flagged** and the confirmation says so, and nothing stops a guest that fills it |
| reserve | host space kept free for macOS (`HARPOON_DISK_HOST_RESERVE`, default 5G) | applies to the two lines above |

So a Mac with 19 GiB free and a 4 GiB disk is offered 8, 16 and 32 GiB (the last one flagged), not only the sizes that could be filled completely.
`HARPOON_DISK_MAX` (default 512G) caps the logical size. Disks never shrink, and a guest template is never used as a disk.

An expansion restarts the VM, so it is refused **before anything is stopped** unless the restart would reproduce the running VM exactly: same
CPUs, memory, disk, kernel and initramfs. A VM started by an older build did not record its kernel and initramfs and cannot be expanded this
way until it has been restarted once; the manual `harpoon stop`, `harpoon disk resize`, `harpoon start` sequence still works. The recovery record
(`disk-expansion.json` in the Harpoon support directory) is written, synced and read back before the VM is stopped; if it cannot be written the
expansion is refused. While an expansion runs, `harpoon status --json` reports `diskExpansion.active`, the desktop disables Start, Stop and
Restart, and the lifecycle commands name the expansion when they are refused. If one is interrupted, `disk status` and `doctor` say how far it got
and what to run, judged from the disk's actual size; growing the file is repeatable and the guest finishes the filesystem on its next boot. There is
no rollback after the disk file has grown. Success is reported only when the file has the requested size, Harpoon is running with Docker ready,
and the guest filesystem reports most of the added capacity.

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
