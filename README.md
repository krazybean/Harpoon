# Harpoon 2.0

*Containers, under control.*

**A lightweight, Docker-compatible local runtime with source-to-deployment workflows for macOS.**

A native macOS workspace for managing Docker-compatible containers, Compose stacks, networks, and development environments without Docker Desktop, and for deploying applications from source onto that same local runtime. Harpoon uses Apple's `Virtualization.framework` and a minimal Linux VM while keeping Docker Engine authoritative for container workloads.

[![Release](https://img.shields.io/github/v/release/krazybean/Harpoon)](https://github.com/krazybean/Harpoon/releases/latest)
[![CodeQL](https://github.com/krazybean/Harpoon/actions/workflows/codeql.yml/badge.svg)](https://github.com/krazybean/Harpoon/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/krazybean/Harpoon/badge)](https://securityscorecards.dev/viewer/?uri=github.com/krazybean/Harpoon)
[![License](https://img.shields.io/github/license/krazybean/Harpoon)](https://github.com/krazybean/Harpoon/blob/main/LICENSE)
[![macOS](https://img.shields.io/badge/macOS-Apple%20Silicon-black?logo=apple)](https://github.com/krazybean/Harpoon)

![Harpoon Deployments overview: a running release with a captured preview of the application, its local URL and the stages of the latest deployment](assets/screenshots/deployments-overview.png)

*Deployments → Overview for an application deployed from a local Git repository: the running release with a captured preview of the app, its local address, and the latest deployment's stages (source, build, preliminary check, cutover, final check) with the recorded health evidence.*

> **Release status.** The container runtime and desktop workspace and the source-deployment features shown above (generated build recipes, Metrics, release-aware cleanup, the redesigned Overview and Settings) are in the [v2.0.0-rc.4 testing prerelease](#downloads-stable-versus-testing). The shipping desktop app has not been launched in acceptance, Intel guest boot is unverified and the two-Mac startup reliability gate is open. See the [rc.4 release notes](docs/releases/v2.0.0-rc.4.md).

## Key capabilities

**Container runtime**

- A lightweight VM-backed Docker Engine, reachable through a standard Docker context (`docker --context harpoon`), without Docker Desktop.
- Container, image, volume and network management in the desktop app, plus Compose stacks.
- Network topology and diagnostics (`harpoon doctor`, the Diagnostics view, read-only discovery of other Docker/Podman endpoints).
- Disk management: grow-only root disk, a confirmed "Reclaim unused space", and confirmed cleanup of old deployment artifacts and build cache.

**Application deployments** (2.0.1 branch; everything runs locally on this Mac)

- Deploy an application from a Git repository registered with Harpoon, or from a local checkout. With no Dockerfile, Harpoon renders a build recipe for npm (`package.json`) and Python (pip or uv) projects; other layouts are refused with a reason, not guessed.
- Build and runtime configuration per application, environment variables, and secrets kept in the macOS Keychain (write-only in the UI; build secrets are BuildKit secret mounts, not image layers).
- Health-checked cutover: a preliminary check before traffic moves and a final check after, with automatic restoration of the previous release if the cutover fails.
- Deployment history, rollback, and "Rebuild & Restore" for a release whose image was cleaned up.
- Live container metrics: real Docker measurements (CPU, memory, network totals, uptime) for the serving container only, with a bounded, session-only, in-memory history.
- Application previews: a private screenshot of the running app on the release card. (Per-branch preview environments are not available.)

Deployments are attended: a person starts each one, and live changes need an explicit confirmation. Harpoon does not deploy on push, host applications in the cloud, or deploy to other machines, and makes no availability guarantees. See [Known Limitations](#known-limitations--v01-scope).

## Screenshots

| | |
|---|---|
| ![Metrics tab: CPU, memory, network and uptime of the serving container](assets/screenshots/deployments-metrics.png) | ![Network topology of containers and Docker networks](assets/screenshots/networks-topology.png) |
| **Container metrics.** CPU, memory (page cache excluded), cumulative network and uptime for the serving container from Docker's own statistics, not the VM. History is bounded to 10 minutes and kept in memory for the session. | **Network topology.** Running containers attached to Docker networks, from Docker's data. Animated links mean attachment, not measured traffic. |
| ![A Compose project workspace with its containers and a cluster-shell prototype](assets/screenshots/containers-compose-detail.png) | |
| **Compose workspace.** Stack status, networks and volumes, a container's published port with the Mac-side listener, and the CSSH cluster-shell prototype. | |

**[Explore all Harpoon screenshots](assets/screenshots/README.md)**

Harpoon runs ordinary Docker development workloads on macOS without requiring Docker Desktop. The host runtime owns the macOS↔Linux boundary; Docker Engine, containerd, and BuildKit remain inside the guest.

## Harpoon 2.0 teaser

[![Watch the Harpoon 2.0 teaser](docs/media/Harpoon-2.0-teaser-v2-thumbnail.jpg)](https://github.com/krazybean/Harpoon/releases/download/v2.0.0-rc.1/Harpoon-2.0-teaser-v2-15s.mp4)

The 15-second [Harpoon 2.0 desktop teaser](https://github.com/krazybean/Harpoon/releases/download/v2.0.0-rc.1/Harpoon-2.0-teaser-v2-15s.mp4) is attached to the [v2.0.0-rc.1 prerelease](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.1) (it is not attached to rc.2 or rc.3). GitHub README pages use a linked thumbnail so the video remains a reliable release download.

## Why Harpoon?

- **Native macOS integration** — built directly on `Virtualization.framework` (no bundled third-party hypervisor).
- **Docker-compatible workflow and API** where actually supported — `docker`, `compose`, and related tools speak to Harpoon's API through a standard Unix socket / Docker context.
- **Deliberately small architecture** — minimal Linux appliance guest, host control plane focused on VM lifecycle, socket bridging, networking, bind-mount transport, and diagnostics.
- **Transparent and inspectable** — source available, documented build from Swift and Tauri, no hidden updater or launch agent.

Harpoon does not replace Docker Engine. Docker Engine, containerd, and BuildKit remain authoritative inside the guest for containers, images, volumes, networks, and builds. Harpoon owns the macOS↔Linux boundary.

## Downloads: stable versus testing

| Release | Status | Downloadable assets |
|---|---|---|
| [**v0.1.1**](https://github.com/krazybean/Harpoon/releases/tag/v0.1.1) | **Latest stable** desktop release | `Harpoon-0.1.1-arm64.dmg`, `SHA256SUMS` |
| [**v2.0.0-rc.4**](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.4) | **Testing prerelease**, not stable 2.0.0 | `Harpoon-2.0.0-rc.4-universal.dmg`, `SHA256SUMS`, `SIGNING-PROVENANCE.txt` |
| [v2.0.0-rc.3](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.3) | Superseded testing prerelease | `Harpoon-2.0.0-rc.3-universal.dmg`, `SHA256SUMS`, `SIGNING-PROVENANCE.txt` |
| [v2.0.0-rc.2](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.2) | Superseded prerelease | **None.** The release page exists but has no attached files. |
| [v2.0.0-rc.1](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.1) | Superseded prerelease | `Harpoon-2.0.0-rc.1-arm64.dmg`, teaser video. Not recommended. |
| [v0.1.0](https://github.com/krazybean/Harpoon/releases/tag/v0.1.0) | Older stable | DMG and the standalone CLI/runtime archive `harpoon-0.1.0-darwin-arm64.tar.gz` |

A standalone v0.1.1 CLI archive is not published. The Releases page is the authoritative list of artifacts.

### v2.0.0-rc.4 (testing prerelease)

- **File:** [`Harpoon-2.0.0-rc.4-universal.dmg`](https://github.com/krazybean/Harpoon/releases/download/v2.0.0-rc.4/Harpoon-2.0.0-rc.4-universal.dmg), 359,820,098 bytes.
- **SHA-256:** `7dd7cbf2a190f192a886419b1ee84cf38c34ad9f557d0d7c9af8a55e45608d51`. This is the only checksum to verify: the hash of the final, notarized and stapled file (`shasum -a 256 Harpoon-2.0.0-rc.4-universal.dmg`, or `SHA256SUMS`).
- **Signing:** Developer ID signed (JUAN G CASTRO, LPX4W69ZH8), notarized by Apple (submission Accepted) and stapled. Gatekeeper accepted a download-style (quarantined) copy of the DMG and the app inside it.
- **Contains:** source deployments and the deployment workspace, and the lifecycle fixes: Docker starts again after a hard stop, and a release confirmed serving restarts with Docker.
- **Packaged for:** Apple Silicon (arm64) and Intel (x86_64), as one universal app. Only Apple Silicon has been run.
- **Requires:** macOS 12 or later.

**Tested.** The runtime packaged in this DMG, in an isolated namespace on Apple Silicon with a fresh disk, booted, reached Docker-ready, ran an Alpine container and recovered Docker on the first start after three consecutive hard kills.

**Not verified.** The two-Mac 60-cycle startup reliability gate is open. The shipping desktop app has not been installed or launched in acceptance, and a source deployment through it was not run on this bundle. Intel guest boot is unverified. Clean-install and upgrade/rollback are incomplete. Intermittent guest faults are unresolved. Existing application containers keep restart policy `no` until `harpoon-deploy restart-policy --apply` is run.

**Installing.** Download the DMG, verify the SHA-256, open the DMG and copy Harpoon to `/Applications`. Gatekeeper is expected to open it without an override; do not disable Gatekeeper. Use disposable containers, and keep a copy of `~/Library/Application Support/Harpoon` and `~/Library/Application Support/com.harpoon.desktop` first.

More detail: [rc.4 release notes](docs/releases/v2.0.0-rc.4.md) and the [release page](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.4).

### Compatibility and removal in rc.3

- **Registration removal ("Remove registration…") and application removal ("Remove application…") are implemented but closed in every shipping build**, rc.3 included; the Settings controls are disabled and show the reason. Application removal v1 would apply only to applications that were never deployed. Nothing in rc.3 enables either operation.
- rc.3 reads deployment-journal schema 1 (every journal today) and schema 2, the format used once a removal has been recorded. rc.3 never writes schema 2, so using it does not change your journal's format.
- **Downgrade limit for a later release.** If a later release that enables removal records one, a journal at schema 2 cannot be opened by a build older than rc.3: it stops with `invalid SQLite record` and leaves the journal unchanged. Move every installation that can open the journal to rc.3 or later before enabling removal.

Source builds remain available for contributors: `bash harpoon/build.sh` (see [Building](docs/building.md)). Use the Releases page above as the authoritative list of published artifacts.

## Desktop workspace

The Tauri/React desktop app is a client of the Harpoon runtime. The [screenshots above](#screenshots) and the [full gallery](assets/screenshots/README.md) were captured from a development build of the 2.0.1 feature branch, not from the rc.3 DMG, so the rc.3 app may differ.

The workspace also includes Overview, Quake Shell, and read-only Context Discovery surfaces, and a Deployments area. Their current behavior and acceptance status are documented in [the UI notes](docs/ui/README.md), [Context Discovery](docs/context-discovery.md), and [the Harpoon 2 roadmap](docs/roadmap.md). The older images under `docs/ui` remain visual references, not runtime screenshots.

## Harpoon 2.0 RC capabilities

The items below were demonstrated on the documented test system (see results), mostly on the source-built CLI and runtime, not by installing the rc.3 DMG. Fresh-start DNS validation has passed — see below.

- **CLI & lifecycle:** `harpoon start` / `stop` / `restart` / `status` / `logs` / `run` / `version` / `help`; background lifecycle via `Process`, single instance via `flock` on `/tmp/harpoon.lock`, PID safety via `proc_pidpath`, stale recovery, terminal independence.
- **Docker integration:** socket at `unix:///tmp/harpoon-docker.sock` (`0600`), Docker context `harpoon` (`harpoon docker setup|status|use`), `docker --context harpoon version/ps/run/build/buildx` works without TCP.
- **Compose & dev workflow:** `compose build/up/down`, bind mounts (`ro` enforcement, named volume `pgdata` persistence), bridge `m9net` + DNS, published ports, `env`/`.env`/`healthcheck`/`scale`/`mem_limit`.
- **Networking:** VZNAT + virtio-net, host loopback forwarder for published ports (`docker run -p 8080:80` → `curl 127.0.0.1:8080`), `/tmp/harpoon-share` VirtioFS bind mounts, `net.ipv4.ip_forward=1`.
- **Installation / distribution:** relocatable `dist/harpoon-0.1.0-darwin-arm64` (historical, v0.1.0) (bin 802K, kernel 33M, initramfs 14M, root 2G sparse), `harpoon/install.sh` to `/usr/local`, `uninstall.sh`, APFS clone-aware `cp -c` provisioning, ad-hoc signing with `com.apple.security.virtualization` for the source-built CLI. (The rc.3 DMG is separately Developer ID signed and notarized; see Downloads.)
- **Desktop workspace:** Tauri 2 + React + Vite with Overview, container table and Compose Stacks, container details, network topology, CSSH cluster shell, Quake Shell prototype, and read-only Context Discovery. Prototype and acceptance-pending surfaces are labeled in the UI and docs.
- **Ecosystem compatibility:** matrix A–H via `harpoon/ec-test.sh` (when host healthy) — compose, contexts, buildx verified; specific matrix rows preserved as `BLOCKED` only by host transient, not product.
- **Validation:** dense acceptance harnesses (M7–M18, EC, UI, D1/D1.1) with logs, `tier-status.csv`, and preserved historical evidence.

Fresh-start DNS validation (clean stop → fresh `harpoon start` → Docker Engine 28.3.3 `linux/arm64` ready → guest `resolv.conf` with working external resolvers → `registry-1.docker.io` resolved → uncached `busybox:1.37` pull succeeded) **now passes**, superseding earlier documentation that described it as pending due to a Virtualization.framework host condition.

## Architecture

Conceptual data flow:

```
Docker CLI / client
        |
Harpoon host socket/API  (unix:///tmp/harpoon-docker.sock → vsock:2375)
        |
macOS Harpoon runtime (harpoon/Sources — Swift, Virtualization.framework)
        |
Virtualization.framework (VZVirtualMachine, VZLinuxBootLoader, VZNAT, VirtioFS, vsock)
        |
Linux VM (Alpine 3.22, Docker Engine 28.3.3, containerd)
        |
Docker Engine / container runtime
```

Harpoon's control plane is deliberately small. Container execution, image management, and orchestration remain inside the Linux VM under Docker Engine.

For component boundaries and transport details, see [Architecture](docs/architecture.md) and [Lifecycle](docs/lifecycle.md).

## Requirements

- To run: macOS 12 or later on Apple Silicon (the tested platform). The rc.4 DMG also contains Intel binaries, but Intel guest boot is unverified.
- To build from source: Xcode Command Line Tools and Rust stable.
- **Docker CLI bundled**, Docker Desktop **NOT required**. Harpoon.app ships Docker's official CLI (pinned in `harpoon/fetch-docker-cli.sh`) and uses it only when no runnable `docker` is installed. On first launch from /Applications, Harpoon asks once for an administrator password to add its bin folder (bundled `docker` and `harpoon`) to `/etc/paths.d/harpoon`, after Homebrew and `/usr/local/bin`, so new Terminal windows find both commands; a declined prompt is not repeated. Docker Compose v2 plugin (`docker compose version`) required only for `compose` workflows. Harpoon discovers a runnable Docker CLI via `PATH` → `/opt/homebrew/bin/docker` (Finder-safe), rejecting stale or incompatible executables, and creates/repairs context `harpoon` → `unix:///tmp/harpoon-docker.sock` without changing your default context (`harpoon docker setup`; desktop ensures on first use). If `~/.docker/config.json` contains `"credsStore":"desktop"` without `docker-credential-desktop`, `harpoon doctor` warns (Harpoon never deletes `credsStore`).
- **Persistent storage**: immutable `assets/guest/harpoon-root.img` template (`2 GiB` sparse, `0/0/0`) → mutable `~/Library/.../harpoon-root.img` (grow-only sparse, never silently replaced). First provision **4 GiB** logical sparse (provisional default; larger workloads such as `azure-sql-edge` exhausted 2G, so Harpoon warns when space runs low and offers a user-confirmed expansion, see `docs/configuration.md`) via `harpoon start --disk-size 16G` or `harpoon config set disk-size`; grow via `harpoon disk resize 16G` (VM stopped, `truncate` + guest `resize2fs`, atomic, `backing>FS` retry).

## Quick Start

Shortest verified path (see [Building](docs/building.md) for prerequisites, packaging, signing, and troubleshooting):

```sh
# Prerequisites: macOS on Apple Silicon, Xcode Command Line Tools, Rust stable
git clone https://github.com/krazybean/Harpoon.git
cd Harpoon

# Build runtime
bash harpoon/build.sh

# Start Harpoon (defaults: cpus 2, memory 4096 MiB)
harpoon/build/harpoon start
harpoon/build/harpoon status
harpoon/build/harpoon docker setup

# Use Docker through Harpoon
docker --context harpoon version
docker --context harpoon run --rm hello-world
docker --context harpoon run --rm alpine:3.22 true

# Stop when done
harpoon/build/harpoon stop
```

After `harpoon/install.sh`, the same commands are available as `harpoon start/stop/status/...` from any directory and without a checkout. Detailed prerequisites and release packaging (Tauri app, DMG) are in [Building Harpoon](docs/building.md).

## Desktop Application

`ui/harpoon-desktop` is a Tauri 2 + React + TypeScript + Vite app. It is a client of the Harpoon runtime: it reads `status --json`, `doctor`, `logs` and `config`, and it starts, stops and restarts the runtime by running the bundled `harpoon` command. The VM itself is owned by the `harpoon` runtime process, not by the desktop window.

Currently implemented views include overview, containers, images, volumes, networks, resources, diagnostics and Deployments. Features include 750 ms bootstrap polling, 3 s periodic polling throttled when hidden, async `counts_cache` coalescing, binary resolver (`HARPOON_BIN` → bundled → `harpoon/build/harpoon` → `/usr/local/bin/harpoon`), and Docker resource counts via the Harpoon socket.

Build in development:

```sh
cd ui/harpoon-desktop
npm ci
npm run tauri dev
```

See [Building](docs/building.md) for release builds (`npm run build:release`).

## Resource Usage / Validation

Representative measurements from the documented test system (Mac15,6 M3 Pro, 18 GiB, macOS 26.5.2, Docker 28.3.3) — observations, not guarantees:

- **Host control plane at idle (30 s, 15 samples):** runtime daemon ~12 MiB RSS, desktop UI ~69–71 MiB RSS, combined ~80–83 MiB. Apple Virtualization.framework XPC service ~86–100 MiB observed separately. See README's earlier “Benchmark note” for full methodology.
- **Earlier feasibility comparison (different methodology, not interchangeable):** Harpoon VM ~386 MiB idle vs Docker Desktop ~954 MiB; under nginx+Redis+Postgres workload Harpoon ~919 MiB vs ~1.76 GiB.

Guest/container memory depends on workload and is not inferable from XPC RSS alone.

Collected evidence under `docs/results` and `harpoon/results` (M13–M18, R1, EC, UI, D1/D1.1) with `tier-status.csv`, `host.csv`, logs, and preserved healthy vs `HOST_VZ_START_FAILURE` windows.

For the repository-level build/test gate used by contributors and pull requests, run:

```sh
sh tools/validate.sh
```

Links: [Performance](docs/performance.md), [Resources](docs/resources.md), [Validation results](docs/results/), [Architecture](docs/architecture.md).

## Security & Trust

Harpoon intentionally exposes inspectable signals rather than claiming security:

- Source available for inspection
- [Security policy](SECURITY.md) with private disclosure guidance
- [Contributing guide](CONTRIBUTING.md)
- Dependency graph enabled, Dependabot alerts (including malware alerts) and security updates enabled
- Dependabot configuration for `npm`, `cargo`, and `github-actions`
- Repository configuration for **CodeQL** (javascript-typescript, rust with `build-mode: none`, Swift using the canonical production runtime build on `macos-latest`) and **OpenSSF Scorecard** (`v2.4.4`, pinned Actions)
- Pinned GitHub Actions (`actions/checkout`, `actions/setup-node`, `github/codeql-action/*`, `ossf/scorecard-action`, `actions/upload-artifact`) to commit SHAs

Harpoon includes repository configuration for CodeQL (javascript-typescript, rust, swift) and OpenSSF Scorecard. The repository is now public.

Automated analysis does not establish that Harpoon is safe, malware-free, or vulnerability-free. See [SECURITY.md](SECURITY.md) for scope, boundaries, and reporting.

## Known Limitations / v0.1 Scope

- **Platform:** Apple Silicon with ARM64 Linux guests is the supported 2.0 release target and the only tested platform. The universal DMG packages Intel binaries and an x86_64 guest, but Intel guest boot has not been verified, so Intel is experimental and unsupported. Windows/Linux hosts are unsupported. Podman/rootless is not a 2.0 gate.
- **Occasional VM startup condition:** An occasional host-side Virtualization.framework VM startup condition has been observed and characterized (see `docs/results/R1.md` for detailed evidence). The same build has been observed to start successfully; the condition is attributed to host/Virtualization.framework state rather than Harpoon packaging. Startup retries are bounded.
- **Distribution signing:** the v2.0.0-rc.4 DMG is Developer ID signed, notarized and stapled. Earlier prereleases and source builds are ad-hoc signed (`com.apple.security.virtualization`) and are not notarized.
- **Deployments:** attended live deployment, replacement, rollback and restore are open in shipping builds, each started by a person and each needing the explicit `--authorize-live-docker-changes` confirmation (the desktop asks first). Unattended execution of queued Git events, registration removal and application removal are closed. Deployments run on this Mac only: there is no cloud hosting, multi-machine deployment, push-triggered deployment, per-branch preview or Development environment, and no availability guarantee. The source-deployment features on the 2.0.1 branch are not in a published release.
- **Updates:** currently planned after a fresh build and restart; live reconfigure is not claimed.
- **Disk:** 2 GiB template + 4 GiB sparse default `harpoon-root.img` (provisional; APFS clone-aware, grown on request via `harpoon disk expand` or `harpoon disk resize`; existing disks keep their size). Bounded at ~962 MiB–1.0 GiB allocated; `growable` was rejected intentionally.
- **Network/FS:** explicit `127.0.0.1` HostIp binding deferred ( `0.0.0.0` → `127.0.0.1` is the safe default); `inotify` host→guest not propagated (documented).
- **Memory reclamation:** virtiomem balloon device and guest driver present, but host-visible reclamation was not consistently demonstrated across the measured tiers; v0.1 does not market balloon as a guaranteed host RSS reduction.
- **Sandbox on this dev host:** `~/Library/Application Support/Harpoon/data` creation is blocked by the host sandbox (`Operation not permitted`); production uses that path, with fallback to `/tmp/harpoon-runtime/data` for tests. Also, no universal performance guarantee beyond the observed environment.

## Documentation

| Document | Purpose |
|---|---|
| [Building](docs/building.md) | Canonical build, packaging, signing, verification |
| [Security Policy](SECURITY.md) | Supported versions, disclosure process |
| [Contributing](CONTRIBUTING.md) | Bugs, PRs, build reference |
| [Roadmap](docs/roadmap.md) | Milestones and current position |
| [Architecture](docs/architecture.md) | Components, boundaries, data flows |
| [Results](docs/results/) | Validation evidence (M13–M18, R1, D1) |
| [Resources](docs/resources.md) | Resource usage and diet limits |
| [UI](docs/ui/README.md) | Visual-language notes and design references |
| [Screenshot gallery](assets/screenshots/README.md) | Current desktop screenshots, by area |
| [Unreleased changes](docs/releases/unreleased.md) | Source-deployment and other work not yet released |

## Status

Harpoon v2.0.0-rc.4 is a testing prerelease, not a stable 2.0.0 release. It is
Developer ID signed, notarized and stapled, and the runtime packaged in it passed
an Apple Silicon Docker smoke test. Still open: installation and first-launch
acceptance of the shipping desktop, Intel guest boot, second-Mac acceptance,
2×30 cold boots, and upgrade/rollback with persistent state preserved. Harpoon
v0.1.1 remains the latest stable desktop release.

The 2.0.0 release is being prepared (version set, [draft notes](docs/releases/v2.0.0.md)); it is not tagged, signed or published, and its clean-install and signing/notarization gates are open.

Artifacts are published on the repository Releases page. Source builds remain supported via `bash harpoon/build.sh` (see [Building](docs/building.md)).

## License

MIT — see [LICENSE](LICENSE).
