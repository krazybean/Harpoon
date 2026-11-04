# Compatibility investigation — buildx independence and loopback-bound port publishing

Status: INVESTIGATION COMPLETE — findings and fix proposals only; **nothing was implemented**
Date: 2026-10-30
Context: two risks found by the deployment baseline
(`docs/results/DEPLOY_BASELINE.md`, PR #103). Evidence classes: **Measured**
(run during this investigation), **Code** (read from the repository), **Docs** (a published
source, not re-verified here), **Inference**, **Not verified**.

No guest asset, release artifact, VM configuration or port-forwarding code was changed.
No binary was downloaded. All experiments used disposable objects labelled
`harpoon.baseline=1` (0 remain) and copies placed in the session scratchpad.

---

## Part A — Buildx independence

### A1. Why the host Docker CLI finds buildx from Docker Desktop

**Measured.** `~/.docker/cli-plugins/` on this Mac holds symlinks created by Docker Desktop
in 2024–2026, for example `docker-buildx → /Applications/Docker.app/Contents/Resources/cli-plugins/docker-buildx`
(also `docker-compose`, `docker-debug`, …). Any Docker CLI that honors
`$DOCKER_CONFIG/cli-plugins` (the default is `~/.docker`) — Homebrew's 29.9.0 at
`/opt/homebrew/bin/docker`, and the CLI Harpoon bundles — resolves `docker buildx` through
those links.

**Measured.** The CLI's other plugin locations are compiled in:
`/usr/local/lib/docker/cli-plugins`, `/usr/local/libexec/docker/cli-plugins`,
`/usr/lib/docker/cli-plugins`, `/usr/libexec/docker/cli-plugins`, plus `cliPluginsExtraDirs`
from `config.json`. The environment variable `DOCKER_CLI_PLUGIN_EXTRA_DIRS` is **not**
honored by CLI 29.9.1 (tested: `unknown command: docker buildx`). On this Mac the system
directory holds only `docker-compose`.

### A2. Does Harpoon's packaging include a usable buildx?

**No (Code).** `harpoon/fetch-docker-cli.sh` downloads Docker's static CLI tarball and
extracts only `docker/docker`; `prepare-bundle.sh` copies that single file to
`Resources/harpoon/bin/docker`; `sign-app.sh`, `verify-bundle.sh` and
`verify-signatures.sh` mention only the Docker CLI. There is no buildx (or Compose) file
anywhere in the bundle, so buildx "works" only when the host happens to provide it.

`docs/docker-integration.md:63` and `README.md:70,75` state that `docker buildx` works
without qualification; that is true only with a host-provided plugin.

### A3. What happens on a Mac with no Docker Desktop plugins

**Measured** with the bundled CLI (29.9.1) and an empty `DOCKER_CONFIG` (this simulates a
clean user configuration; Docker Desktop itself remains installed, so see "Not verified"):

| Command | Result |
|---|---|
| `docker buildx version` | `unknown command: docker buildx` |
| `docker build` (plain Dockerfile) | works, but prints "The legacy builder is deprecated and will be removed" |
| `DOCKER_BUILDKIT=1 docker build` | **refused**: "BuildKit is enabled but the buildx component is missing or broken" |
| `docker build --secret id=s,src=…` | **fails**: unknown flag `--secret` (legacy builder) |

Consequences (Inference from the above plus Docs): on a clean Mac, Harpoon's Docker
compatibility silently degrades to the deprecated builder. BuildKit-only Dockerfile
features (`RUN --mount=type=secret|cache|bind`, heredocs, `# syntax=` frontends) and
build-time secrets — required by Milestone C — do not work. The guest side is fine: the
guest runs BuildKit v0.23.2 (`docker buildx ls` shows `default* docker … running v0.23.2`).

### A4. What makes BuildKit builds work with Docker Desktop completely absent

**Measured (mechanism proof).** A *copy* of the Desktop buildx binary (v0.32.1-desktop.1,
arm64, already on this machine — not downloaded) in a scratch directory:

- Via `config.json` `{"cliPluginsExtraDirs":["<dir>"]}`: `docker buildx version` works and
  `docker build --secret …` succeeds against Harpoon.
- **Standalone, with no Docker CLI involved and an empty `DOCKER_CONFIG`**:
  `docker-buildx build --secret id=s,src=… --load …` succeeds against the Harpoon socket
  (`DOCKER_HOST=unix:///tmp/harpoon-docker.sock`), using the default `docker` driver and
  BuildKit v0.23.2, and the image is loaded.

So the minimal requirement is a **pinned buildx binary shipped in the bundle and invoked
directly**; no plugin discovery, no edit to the user's `~/.docker/config.json`, no
dependency on Docker Desktop.

**Not verified:** a machine that genuinely has no Docker Desktop (this one does, so
absence was simulated by configuration only); buildx **v0.38.1** (the current upstream
release) against the guest's BuildKit v0.23.2 / Engine 28.3.3 — only the Desktop 0.32.1
build was exercised.

### A5. Separate arm64 and x86_64 binaries?

**Docs (checked via release metadata and checksums, not by downloading binaries).**
Upstream buildx v0.38.1 (published 2026-10-20) provides separate Mach-O files:

| Asset | Size | SHA-256 (release `checksums.txt`, matches the GitHub asset digest) |
|---|---|---|
| `buildx-v0.38.1.darwin-arm64` | 65.5 MB | `6f20ba2437865311ef99015bf5d1039c3c8e4d2177c1c64aa41b134b5ecd3012` |
| `buildx-v0.38.1.darwin-amd64` | 70.6 MB | `76e7a809cc98d34a4c1b7db23feac673def5b8dcdddd9e713ef26598689aa796` |

Each release also publishes SLSA provenance and SBOM JSON per asset and a
`checksums-signed.txt`. Signature verification of those attestations was **not** performed.

- **Apple Silicon-only 2.0:** ship the arm64 file only (+65.5 MB).
- **Universal (experimental):** the existing Docker CLI is `lipo`-merged for universal
  builds; do the same, or ship both slices side by side. Either adds ≈ 136 MB to a bundle
  whose universal DMG is already 288 MB. A per-arch download at first use was
  rejected here: it needs network, a trust decision, and verification before first build.

### A6. Smallest maintainable change (proposal)

Mirror the existing Docker CLI mechanism; touch nothing else.

1. **`harpoon/fetch-buildx.sh`** (new, ~40 lines, modeled on `fetch-docker-cli.sh`): pinned
   `BUILDX_VERSION`, per-arch SHA-256, download to a cache, **fail on mismatch**, output
   `harpoon/build/docker-buildx` (lipo'd when `HARPOON_ARCH=universal`).
2. **`prepare-bundle.sh` / `build.rs`**: stage it as
   `Resources/harpoon/bin/docker-buildx` next to `docker` (same two call sites that stage
   the CLI today).
3. **`sign-app.sh`**: add a block identical to the Docker CLI block (`--options runtime
   --timestamp`, Developer ID, no entitlements); **`verify-signatures.sh`** and
   **`verify-bundle.sh`**: existence, executable bit, architecture, secure timestamp.
4. **Resolution rule (engine and `harpoon` CLI):** Harpoon-managed builds call the bundled
   `docker-buildx build` directly — deterministic and independent of the user's PATH or
   `~/.docker`. If the file is missing, fail with a clear message instead of falling back
   silently to the legacy builder.
5. **Docs:** qualify the buildx claims; add the license notice (buildx is Apache-2.0) to
   the bundle's notices.

Out of scope for the smallest change (listed so they are not forgotten): making
`docker buildx` work in the user's Terminal when the bundled `docker` is on `PATH`
(`cli_path.rs`) — that needs either a plugin directory the CLI searches or a
`cliPluginsExtraDirs` entry in the user's config, which modifies user configuration and
should be an explicit decision; and Docker Compose, which is missing from the bundle for
the same reason.

### A7. Tests and verification

| # | Test | Needs |
|---|---|---|
| 1 | `fetch-buildx.sh` rejects a tampered download (wrong SHA-256) and a missing slice | no VM |
| 2 | `verify-bundle.sh`: `bin/docker-buildx` exists, executable, correct archs (arm64; + x86_64 when universal) | built app |
| 3 | `verify-signatures.sh`: Developer ID, hardened runtime, secure timestamp on `docker-buildx` | signed app |
| 4 | `docker-buildx version` runs from the bundle with `env -i`, empty `HOME` and `DOCKER_CONFIG` | built app |
| 5 | **Hermetic build test** (new `tools/verify-buildx-build.sh`, live Harpoon): from the bundle, with `env -i PATH=/usr/bin:/bin HOME=<tmp> DOCKER_CONFIG=<tmp>`, run `docker-buildx build --load` for (a) a plain Dockerfile, (b) `--secret` + `RUN --mount=type=secret`, (c) `RUN --mount=type=cache` and a heredoc; assert the images exist and the secret is absent from `docker history`. Negative control: the legacy `docker build --secret` fails | live VM |
| 6 | Version-compatibility check of the pinned buildx against the guest's BuildKit | live VM |
| 7 | Engine unit tests for the resolution rule with a fake filesystem (bundled present / absent / not executable) | none |
| 8 | **Clean-machine gate:** run 5 on a Mac (or fresh macOS user plus Docker Desktop uninstalled) with no Docker Desktop at all | second Mac |

### A8. Release implications

- **Size:** +65.5 MB arm64; +136 MB universal.
- **Signing/notarization:** a new nested Mach-O must be signed with the Developer ID,
  hardened runtime and secure timestamp, and be covered by notarization — otherwise
  Gatekeeper/notarytool rejects the bundle. Its upstream signature state was not
  checked (**Not verified**); `codesign --force` replaces it regardless.
- **Supply chain:** pin version and SHA-256 in-repo; the digests above are corroborated
  by two upstream sources, but attestation verification is not done. Bump with the Docker
  CLI pin.
- **Licenses:** add buildx's Apache-2.0 notice.
- **RC.2:** untouched. This is not a release-blocking defect for the Docker-compatible
  core on machines that already have buildx, but it **is** required for Milestone C
  (BuildKit secrets) and for BuildKit-syntax Dockerfiles on clean Macs.
- **Severity:** P2 for 2.0 (conditional: depends on whether clean-Mac BuildKit builds are
  part of the 2.0 compatibility promise); P1 for Milestone C.

---

## Part B — Publishing with `-p 127.0.0.1:PORT:CPORT`

### B1. Root cause

**Code.** `PortForwardManager.sync()` reads `GET /containers/json` and, for every running
container's published TCP port, creates a host listener on **`127.0.0.1:PublicPort`**
(`hostAddrForDockerIP` returns `127.0.0.1` for every Docker bind address — "do not expose
LAN") that connects to **`<guest NAT IP>:PublicPort`** over VZNAT. The Docker `IP` field of
the port mapping is read but used only to choose the host listener address; it is never used
to choose the guest destination.

**Measured (guest).** With `-p 127.0.0.1:38210:8080`:

```
/usr/bin/docker-proxy -proto tcp -host-ip 127.0.0.1 -host-port 38210 -container-ip 172.17.0.2 -container-port 8080
-A DOCKER -d 127.0.0.1/32 ! -i docker0 -p tcp -m tcp --dport 38210 -j DNAT --to-destination 172.17.0.2:8080
tcp 0 0 127.0.0.1:38210 0.0.0.0:* LISTEN
net.ipv4.conf.{all,docker0}.route_localnet = 0
```

The guest listener and the DNAT rule match only destination `127.0.0.1`; connecting to the
guest NAT address `192.168.64.3:38210` has no listener. The runtime log shows the result:
`HOST_FORWARD_CONNECT_FAILED … 192.168.64.3:38210 … Connection refused`.

### B2. Behavior matrix

**Measured**, 6 publish forms, host loopback and host LAN address (`192.168.1.109`) tested
from macOS, and loopback and NAT address from inside the guest:

| Publish form | Docker-reported bind | macOS `127.0.0.1` | macOS LAN IP | Guest `127.0.0.1` | Guest NAT IP |
|---|---|---|---|---|---|
| `-p P:8080` | `0.0.0.0` **and** `::` | **works** | no | ok | ok |
| `-p 127.0.0.1:P:8080` | `127.0.0.1` | **fails (refused)** | no | ok | **fails** |
| `-p 0.0.0.0:P:8080` | `0.0.0.0` | **works** | no | ok | ok |
| `-p [::]:P:8080` | `::` only | **fails** | no | fails | fails |
| `-p [::1]:P:8080` | `::1` | **fails** | no | fails | fails |
| `-p <guest NAT IP>:P:8080` | `192.168.64.3` | **works** | no | fails | ok |

(The unusual guest-NAT-IP form works only because the forwarder's guest destination happens
to equal it.) In every case the host LAN address is unreachable: **Harpoon never exposes a
published port beyond host loopback** (Measured and Code).

### B3. Implications

- **Localhost-only exposure.** The *intent* of `127.0.0.1:` — "reachable only from this
  machine" — is already what Harpoon provides for every publish form. The failing forms
  break availability, not isolation. Developers copying Docker Desktop or production
  Compose files with `127.0.0.1:` (very common) get a listener that refuses connections.
- **Guest NAT binding.** Publishing as `-p P:C` binds the port on the guest's NAT address
  (`192.168.64.x`) as well as guest loopback. That address is reachable from macOS (and
  other local VZNAT peers). *Inference:* another local user or process on the same Mac
  could reach it, which is the same exposure class as Docker Desktop's loopback ports.
- **Host LAN accessibility.** Docker Desktop (Docs; **not verified here — Desktop is not
  running, and I did not start it**) binds `-p P:C` on all host interfaces and `-p
  127.0.0.1:P:C` on loopback only, so `-p P:C` is LAN-reachable there. Harpoon is stricter
  by design. Anyone relying on LAN reachability gets none, and there is currently no opt-in.
- **Deployment previews / production-like environments.** Previews are loopback URLs by
  decision (requirements D1/D7). The deployment engine must publish as `-p PORT:CPORT`
  **without a host IP** until the forwarder is corrected; it must not translate a user's
  `127.0.0.1:` into anything else on its own. Any "preview shared on the LAN" feature needs
  a separate, explicit product decision and a deliberate LAN bind.
- **Backward compatibility.** Existing `-p P:C` and `0.0.0.0` behavior must not change.
  Containers *created* with a loopback bind before a fix keep their loopback-only Docker
  binding (a restart does not rewrite it) and must be recreated.

### B4. Candidate corrections

| Option | What changes | Cost / risk |
|---|---|---|
| **1. Rewrite the bind address at container creation** in the Docker API bridge: in `POST /containers/create` bodies change `HostConfig.PortBindings[*].HostIp` of `127.0.0.1`, `::1`, `::` to empty (all), so Docker binds on guest all-interfaces; Harpoon's listener stays loopback-only. Optionally record the original in a label. | Host Swift only (`HostPathTranslator.translateCreateBody` already does an analogous narrow rewrite of `Binds`/`Mounts`); no guest asset change | Changes `docker inspect` output (`HostIp` no longer `127.0.0.1`); containers created before the fix are unaffected; must handle Compose (same API), chunked bodies and unknown fields without corrupting JSON |
| 2. Reach guest loopback from the host | A guest-side relay (vsock or `socat`) that connects to `127.0.0.1:PORT` inside the guest | **Guest asset change** and a new privileged component; out of bounds now |
| 3. Warn instead of fix | `doctor` and sync log flag loopback-bound mappings; docs | Does not fix the failure; useful as an immediate mitigation |

### B5. Recommendation

**Option 1, preceded by Option 3.** Rationale (Inference): it fixes the documented failing
form at the single point where Harpoon already normalizes create requests, adds no guest
component, and preserves Harpoon's loopback-only host exposure.

Normalization rules to specify before coding:

1. Rewrite only `HostIp` values `127.0.0.1`, `::1` and `::`. Leave empty, `0.0.0.0` and
   explicit non-loopback addresses unchanged.
2. Leave everything else in the body byte-for-byte unchanged when nothing matches (no
   re-serialization on the unchanged path).
3. Decide and document whether to keep the original value in a label (for example
   `io.harpoon.requested-host-ip`) so `docker inspect` consumers can see intent.
4. Do not alter UDP behavior (still unsupported) or ranges.

### B6. Regression tests

| # | Test | Needs |
|---|---|---|
| 1 | Pure-function fixtures for the rewrite: loopback → empty, `::1`/`::` → empty, `0.0.0.0` and empty unchanged, non-loopback explicit unchanged, multiple bindings, missing/`null` `PortBindings`, malformed JSON passes through untouched (extends `regression-host-path.swift`) | no VM |
| 2 | Live matrix (the six forms in B2) as a script under `harpoon/`: expect macOS-loopback reachable for **all** forms after the fix, LAN address unreachable for all | live VM |
| 3 | Compose `ports: ["127.0.0.1:P:C"]` reachable from macOS | live VM |
| 4 | Existing `-p P:C` behavior and timings unchanged (re-run the 30-sample publish measurement; median must stay ≈ 0.64 s) | live VM |
| 5 | A container created before the fix with a loopback bind: `doctor` reports it as "loopback-bound, unreachable from macOS; recreate" | live VM |
| 6 | Negative: `HostIp` rewrite does not occur for non-create requests | no VM |

---

## Decisions required before implementation

1. **Buildx:** include the bundled buildx in 2.0 / RC.3, or defer to Milestone C? Accept the
   size cost (+65.5 MB arm64, +136 MB universal)? Which version to pin (upstream v0.38.1
   needs a compatibility run against BuildKit v0.23.2 first)?
2. **Terminal `docker buildx`:** leave unsupported for bundled-CLI users, or approve a
   plugin directory / `cliPluginsExtraDirs` change to user config?
3. **Port forwarding:** approve Option 1 (bridge rewrite) and whether to keep the original
   bind address in a label?
4. **LAN publishing:** confirm Harpoon stays loopback-only for now, or plan an explicit
   opt-in.

## Not verified / limits

Docker Desktop was not started, so its port-binding behavior is cited, not observed; a
Mac with Docker Desktop truly absent was not available; buildx v0.38.1 was not run;
release attestations were not verified; results are from one Mac and one session.
