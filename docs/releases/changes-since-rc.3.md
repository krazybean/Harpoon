# Changes since v2.0.0-rc.3 (released in rc.4)

Detail for changes merged to `main` after [rc.3](v2.0.0-rc.3.md). They shipped in the [rc.4 prerelease](v2.0.0-rc.4.md) and are not in the rc.3 DMG. The sections below describe **already released** rc.4 features, not pending changes. This file was formerly `unreleased.md`. It is the historical rc.4 change log; use the [rc.4 release notes](v2.0.0-rc.4.md) for release status. Changes made after rc.4 are listed in [unreleased.md](unreleased.md), not here.

## Behavior changes

### Applications come back after a Harpoon restart (shipped in rc.4)

- **Cause.** Harpoon created every application container with restart policy `no`, so after any guest restart the applications stayed
  down until started by hand. The database container (a Compose service with `unless-stopped`) came back; the applications did not.
- **Fix.** A release that is confirmed serving is set to `unless-stopped`, after its success is durable in the journal. Candidates and
  failed releases stay `no`. Releases that were stopped on purpose, including the one a replacement stopped, stay stopped.
  `harpoon-deploy restart-policy` (dry run by default) applies the same to releases that predate this. See
  [Application recovery after a restart](../deployment-recovery.md).

### Source deployments: a repository without a Dockerfile builds from a recipe Harpoon generates (shipped in rc.4)

- **Detection.** When a registration names no Dockerfile and the checkout has none, Harpoon detects an npm project (`package.json`;
  `npm ci` with a lockfile) or a Python one (pip or uv only) and renders a Dockerfile for that one build. It exists only in the private
  build snapshot, never in the project folder. Other package managers are refused with a reason, not guessed.
- **Settings.** Install, build and start commands, Production or Development style, and a Docker network are stored with the registration
  (an optional trailing field on `gitrepo`; a registration without settings is byte-identical to before) and shown in the Build panel. Jobs are
  not implemented and are refused rather than run as Services.
- **Secrets stay out of the image.** Build-flagged secrets are BuildKit secret mounts on the install and build commands only.
- **No per-application caps.** New containers no longer get 256 MiB / 0.5 CPU / 64 PIDs; the VM's own safeguards still apply. A Node install
  and build run with a 2 GiB heap bound (`ARG`, build time only).
- **Pulls.** A generated build's base image (`node:<major>-slim`, `python:<version>-slim`) is fetched by the builder when it is not cached. A Dockerfile build still refuses a base image that is not local.
- **Release-aware cleanup.** `cleanup-releases` (dry run by default) removes the stopped containers, port leases and images of releases that no
  longer serve, and the retained images of failed deployments, each by its recorded id with the ownership labels re-checked, and journals
  every removal. It never touches the release serving now (or the image that release pins), an unfinished or cancelled deployment, volumes,
  networks, build cache or other applications. A deployment run through `git-deploy` runs it when it ends; a failure there is a note, never a
  failed deployment. Reclaim Unused Space lists the same items first and removes them only after confirmation, and reports the guest's measured
  free space before and after. `prune-build-cache` (dry run by default) is Docker's own prune of unused BuildKit cache, separately confirmed.
- **Rebuild & Restore.** A release whose image was cleaned up is restored by rebuilding its recorded commit in a private checkout through the
  normal deploy and cutover (current build settings and Variables apply, so the result can differ from the original). The Roll back button
  keeps its meaning for a release whose image is still present.
- **Preview.** An app whose Content-Security-Policy includes `upgrade-insecure-requests` cannot load its scripts and styles in WebKit over http, even on
  loopback. Its private capture goes through a loopback-only, GET-only, cookie-free relay (`hpcapture://`) that removes only that one directive from
  what the capture window sees and refuses redirects off the app's own host; the app's real headers and what ordinary visitors get are untouched. The
  capture waits for a drawn, settled frame, refuses a blank one, runs one window at a time and closes it on success and failure.

### Deployments: the Metrics tab shows the serving container's real measurements (shipped in rc.4)

- The tab replaces its placeholder with CPU, memory, network and uptime for the container the journal says is serving, read
  from Docker's own API through the existing socket (`GET /containers/{id}/json` and `/stats`; nothing is spawned, nothing
  is changed). Before any measurement the container must carry Harpoon's labels for this application and deployment and
  match the recorded container id; a container that merely has the recorded name is never measured.
- Figures Docker does not provide stay empty rather than zero. CPU is a percent of one core (it can exceed 100% on several
  cores). Memory excludes the page cache when Docker reports it, and a percentage appears only when the container has its
  own memory limit. Rates need two samples of the same run; a counter reset or a restart gives none.
- Sampling is about every 3 seconds, only while the tab is open and the window visible. A reading older than about 10
  seconds is shown as stale, not current. Up to 10 minutes of history is kept in memory for the session only; it resets when
  the serving container changes. No telemetry is stored.

### VM-backed tests run under a deadline and a watchdog (test tooling only)

Scripts that boot or drive a real VM now re-run under `tools/vm-test-guard.py`: wall-clock deadline, preflight and 15 s sampling of
free disk / memory pressure / current swapping, ownership by an inherited per-run marker and exact (pid, start time) identity (never a
pre-existing VM, the runner, or an unrelated process), exclusive use of the production namespace (a running engine makes the run refuse
unless `VMGUARD_ADOPT_EXISTING=1`), a supervised detached reaper, a `--sweep` for orphans, and bounded failure diagnostics. Production VM uptime is not limited. See
`docs/testing-vm-containment.md`, including what cannot be guaranteed.

### A guest can no longer exhaust the host through its console or memory (hardware-validated once; see the [v2.0.0 notes](v2.0.0.md))

- **Guest console.** The serial console is read from a pipe by a dedicated thread, scanned for readiness, failure and kernel-panic
  markers as bytes (split reads, binary and malformed UTF-8 are handled), and kept in a rotating two-file log of at most 8 MiB
  (`harpoon-serial.log`, `harpoon-serial.log.1`) written at a limited rate (2 MiB burst, then 64 KiB/s). A console flood is still
  drained and scanned, its excess is counted and noted in `harpoon.log` (`HARPOON_SERIAL_OVERFLOW`), and **a healthy guest is never
  stopped because its console is noisy or old**. Previously the whole log was re-read every second and could grow without limit. Only the
  newest output is retained, so very early boot lines are rotated out of a flooded log.
- **Failures are recorded, not silent.** `serial_storage` (the log cannot be written; the guest is unaffected) and `serial_collector`
  (the reader stopped; console output could back up into the guest) appear as the runtime failure in `status --json`.
- **Memory admission.** `harpoon start`/`run` now reserves guest memory in a host-wide registry (`/tmp/harpoon-admission-<uid>`),
  shared by every `HARPOON_TEST_TMPDIR` namespace. Each guest counts its memory plus 512 MiB; the total may not exceed 60% of physical
  RAM, otherwise the start is refused (exit 11) with the numbers. Reservations are flock-held, so a crashed engine's reservation is
  reclaimed automatically, and concurrent starts are serialized. A registry that cannot be made private refuses the start.
- **Guest init.** The final idle loop no longer spins if `sleep` itself fails: it reports `HARPOON_INIT_SLEEP_FAILED` once, paces with a
  shell builtin, and parks after 20 straight failures. PID 1 never exits. The in-tree arm64 initramfs was rebuilt with this loop
  (sha256 `58420573…`; `guest-assets.manifest` and `.inputs` updated). The x86_64 initramfs comes from the older guest-assets tarball and does not include it.
- The underlying `sleep`/PID 1 faults (SIGILL / SIGSEGV) are still unexplained and intermittent; see the [v2.0.0 notes](v2.0.0.md#known-issue-unexplained-intermittent-guest-faults).

### New root disks start at 4 GiB, and Harpoon offers to grow them (provisional)

A **new** root disk is 4 GiB logical (sparse) instead of 32 GiB. Disks that already exist are not touched, and an explicit
`--disk-size`, `HARPOON_DISK_SIZE` or `disk-size` still wins. **The 4 GiB default is provisional:** the guest uses about 335 MiB and has run
Docker at the 2 GiB template size, and a live boot at a 4 GiB disk was recorded once (the 2026-11-13 reliability gate, [v2.0.0 notes](v2.0.0.md)), but some workloads need more (`azure-sql-edge`
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
  Port sync no longer stays in flight forever behind a connect that never returns. A stale `HARPOON_RUNNING` log line no
  longer makes Docker or the engine look ready when the live probe fails or cannot connect; `status --json` reports that
  case as `dockerUnverified: true` with `dockerReady: false`.
- A guest kernel panic is now surfaced. The host gets no stop callback for one, so the runtime watches the guest console and,
  on the kernel's panic line, logs `HARPOON_GUEST_PANIC` and records a `guest_panic` failure that `status` and the desktop banner
  show. It does not restart or stop anything. Known and unresolved: the guest's PID 1 (the init shell script) died of SIGSEGV
  in the observed panic; the cause needs a separate investigation, and `panic=1` was deliberately not added.

## Documentation

- The help text, usage line and configuration guide now state the real precedence for `cpus` and `memory`: CLI flag >
  environment > config file > default (#181, #182).
