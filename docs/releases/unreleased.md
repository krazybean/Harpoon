# Unreleased changes (after v2.0.0-rc.3)

Changes merged to `main` since [rc.3](v2.0.0-rc.3.md) that are not in any published release. They are not in the rc.3 DMG.

## Behavior changes

### New root disks start at 4 GiB, and Harpoon offers to grow them (provisional)

A **new** root disk is 4 GiB logical (sparse) instead of 32 GiB. Disks that already exist are not touched, and an explicit
`--disk-size`, `HARPOON_DISK_SIZE` or `disk-size` still wins. **The 4 GiB default is provisional:** the guest uses about 335 MiB and has run
Docker at the 2 GiB template size, but a live boot at 4 GiB has not been recorded yet, and some workloads need more (`azure-sql-edge`
exhausted 2 GiB). To keep the old behavior, set `HARPOON_DISK_SIZE=32G` or `harpoon config set disk-size 32G` before the first start.

While Harpoon runs, the desktop warns when the Docker disk passes 80% (or 1 GiB free) and again at 92% (or 384 MiB free), and offers larger
sizes. Choosing one asks for confirmation, then stops Harpoon, grows the disk, starts it and verifies the filesystem and Docker. Running
containers are interrupted. Nothing resizes or restarts without that confirmation. Headless: `harpoon disk status [--json]` shows the
warning and `harpoon disk expand <size> --yes` performs it (no prompt; without `--yes` it changes nothing). See `docs/configuration.md`.
A guest template can no longer be attached as a disk or resized.

### Disk size for a new root disk follows the same order as cpus and memory (#184)

`CLI flag > environment > config file > built-in default`: `harpoon start --disk-size`, then `HARPOON_DISK_SIZE`, then the
`disk-size` key in `config.json`, then 4 GiB (was 32 GiB; see below).

**If you relied on the config file winning:** until now `harpoon config set disk-size 16G` overrode `HARPOON_DISK_SIZE`.
It no longer does. If `HARPOON_DISK_SIZE` is set in your environment (a shell profile, a launch agent, a script), it now
decides the size of the next new disk, and the config value applies only when the variable is unset. To keep using the
config file, unset the variable.

- The size applies only when a disk is **created**. An existing disk is never resized, truncated or replaced by any of these;
  use `harpoon disk resize` to grow it.
- A value that is not a size, or is below 2 GiB, is skipped and the next layer applies (previously an unparseable
  environment value was also ignored, but one below 2 GiB produced a template-sized disk). `--disk-size` and
  `harpoon config set disk-size` still reject such values with an error.
- The default is now 4 GiB, sparse (see the next section).

### Inspecting the root disk no longer creates it (#183)

`harpoon status --json`, `doctor`, `diagnostics`, a rejected `start`, and any command run with `HARPOON_DISK` or `--disk` used
to create the default root disk as a side effect. The disk is now created only when a VM is about to start for the first
time, and never when an explicit disk path is given. `status --json` can therefore report a `diskPath` that does not exist yet
before the first start, and `doctor` shows it as a warning (not yet created) instead of a pass.

### `doctor` and `disk status` stay inside a test namespace (#186)

With `HARPOON_TEST_TMPDIR` set, a namespace that has no root disk of its own made `doctor` and `disk status` report the
production disk (`~/Library/Application Support/Harpoon/data/harpoon-root.img`). They now report `<root>/data/harpoon-root.img`
as not yet provisioned. Read-only before and after; nothing is created. Without the variable nothing changes.

### `doctor` no longer warns about normal ext4 overhead, and the desktop shows the disk image size

`doctor` compared the backing file with the guest's `df` capacity using a 2% ceiling, but ext4 on the shipped layout keeps about 3 to 5%
for the journal and inode tables, so a healthy 4 GiB disk warned. The ceiling is now 6%; a filesystem that is genuinely short of the file
(an unfinished expansion) still warns. `harpoon status --json` now reports `diskLogicalBytes`, so the desktop's *Disk image* tile no longer
reads "Unavailable". The field is absent when the disk file does not exist yet; older responses without it still render as before.

### Reclaim unused disk space, and stage-by-stage expansion progress

`harpoon disk reclaim --yes` (desktop: Resources → *Reclaim unused space…*) runs `fstrim /` in the running guest after explicit confirmation and
reports the measured change in the image's APFS allocation, including zero. It does not delete or prune Docker data, restart Harpoon or change
the disk's capacity. The disk-expansion panel now shows the real stage (stopping, growing, starting, verifying) from the recovery record, also
when the desktop reconnects mid-expansion, clears after a verified success, and keeps the recovery instructions if an expansion is interrupted.

### The desktop follows the runtime after an external stop/start or disk expansion

After Harpoon was stopped and started from a terminal (including `harpoon disk expand`), the desktop kept its "stopped" bootstrap state: the
header said running while the Overview showed "Harpoon is stopped" with an enabled Start button, and Refresh did not help. A status that says
running with Docker ready now always moves the desktop to ready (also out of a stale "stopped" or timed-out "failed"), an externally started VM
is followed through its boot stages, and the Start panel and button are never offered for a running, starting, stopping or expanding VM.

### Bridge connections no longer pile up when a client gives up, and the desktop startup cannot hang on one

The Docker and management bridges closed a connection only when both sides had ended. A client that gave up (for example a status probe that
timed out) while the guest stayed silent left both descriptors open for as long as the guest said nothing, so a guest whose management
service did not answer collected two descriptors per probe. Each connection is now owned by one object on one serial queue: descriptors are
closed only after their dispatch sources are cancelled, a client that is gone entirely is detected (a client that only half-closed still gets its
answer), a blocked write waits without a busy loop and ends as soon as the client is gone, connects that never answer time out after 10 s, the
number of open connections is bounded (256 Docker, 32 management) and shutdown closes them, and a listener that runs out of descriptors pauses
instead of spinning. The desktop startup overlay now gives way as soon as the runtime reports running with Docker ready (after 10 s at most),
each startup read is time-bounded, and a sustained management outage is shown as a warning rather than a hang.
- A dead guest no longer looks like a ready Docker. `harpoon status` now counts Docker as ready only when `GET /_ping` is
  answered through the socket (a bridge to a crashed guest still accepts connections). After three vsock connect failures
  the Docker bridge, the management bridge and port sync stop calling the Virtualization framework (which cannot cancel a
  connect and keeps a descriptor for each one that never completes) and try one probe every 15 s until the guest answers.
  Port sync no longer stays in flight forever behind a connect that never returns.

## Documentation

- The help text, usage line and configuration guide now state the real precedence for `cpus` and `memory`: CLI flag >
  environment > config file > default (#181, #182).
