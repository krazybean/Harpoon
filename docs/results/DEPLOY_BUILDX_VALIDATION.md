# Buildx independence — validation against Harpoon's guest BuildKit

Follow-up to `COMPAT_BUILDX_AND_PORT_FORWARDING.md` (Part A). That investigation exercised only the Docker Desktop
copy of buildx (v0.32.1-desktop.1). This one tests the **pinned upstream standalone buildx v0.38.1** the proposal
names, run in isolation. Date: 2026-10-30. Host: Apple Silicon, macOS, Harpoon runtime already running.

## Verdict
**The pinned standalone buildx v0.38.1 works against Harpoon's guest BuildKit v0.23.2 (Engine 28.3.3) with no
Docker Desktop, no CLI, no plugin directory and an empty `HOME`/`DOCKER_CONFIG`.** Plain, `--mount=type=secret`,
`--mount=type=cache` and heredoc builds succeed. Two gates remain open (below); neither is a buildx defect.

## What was run (all read-only or additive-and-transient; nothing pruned, removed or modified)
Every command used `env -i PATH=/usr/bin:/bin HOME=<empty tmp> DOCKER_CONFIG=<empty tmp>
DOCKER_HOST=unix:///tmp/harpoon-docker.sock` and the downloaded binary directly (no `docker` involved).

| # | Test | Result |
|---|---|---|
| 1 | Download v0.38.1 darwin-arm64 and darwin-amd64; SHA-256 vs the pins in the compat doc **and** vs GitHub's per-asset digest | both match (`6f20ba24…3012`, `76e7a809…a796`) |
| 2 | `codesign --verify --strict`; authorities, flags, timestamp | valid; **Developer ID Application: Docker Inc (9BNSXJN65R)**, hardened runtime (`flags=0x10000`), secure timestamp 2026-10-20, both slices |
| 3 | `buildx version` with `env -i` and an empty `HOME` | `v0.38.1 0dbe87f3…`; only creates `<HOME>/buildx` (the scratch dir) |
| 4 | amd64 binary under Rosetta (`arch -x86_64 … version`) | runs; **not** exercised against a guest (the guest is arm64-only here) |
| 5 | `buildx ls` / `inspect` against Harpoon | default `docker` driver, **BuildKit v0.23.2**, `linux/arm64`, GC policy readable |
| 6 | (a) plain `FROM scratch` build, `--output type=cacheonly` | success |
| 7 | (b) `RUN --mount=type=secret` + `--mount=type=cache` + heredoc, local `alpine:3.22`, no `# syntax=` line | success; assertions inside the build confirm the secret content, the cache mount and the heredoc |
| 8 | Negative control: same build with `--no-cache` and a **wrong** secret | fails (exit 1), so the assertion has teeth |
| 9 | Secret hygiene: unique secret value searched in build logs and in `buildx du --verbose` | 0 hits in both |
| 10 | Negative control: legacy `docker build --secret` | not accepted by the legacy builder, as documented |
| 11 | Resource accounting before/after | images 6→6, containers 9→9, volumes 1→1; BuildKit cache reported 367 MB before and after at display precision (a few KB of new records are possible and were **left in place**; nothing pruned) |

Notes on the test design: the first secret test embedded the expected value in the Dockerfile and a cached step made
a wrong secret "pass" — BuildKit's cache key does not include secret *content*. Both artifacts were test mistakes,
fixed above, and the finding is real: **an engine that relies on secrets must disable the build cache for any step
whose secret changed**, or version the secret in a build arg that is part of the cache key (Milestone C concern).
During the DNS probe one `docker run --rm --pull=never alpine:3.22 nslookup …` container was created and
auto-removed.

## Packaging options
| Option | Verdict |
|---|---|
| **A. Bundle both pinned binaries (arm64 + amd64) side by side or `lipo` them** | Recommended. Mirrors the existing Docker CLI mechanism. `lipo` universal = 136.1 MB (arm64 65.5 MB, amd64 70.6 MB). |
| B. Apple-Silicon-only bundle (arm64) | Smallest: +65.5 MB. Right for the 2.0 primary artifact; the Intel/universal DMG stays experimental and ships the lipo'd binary. |
| C. Download at first use | Rejected: needs network and a trust decision before the first build. |
| D. Build buildx from source in CI | Not now: adds a Go toolchain and reproducibility work for no gain over the signed upstream binary. |

## Verification and supply chain
- **Pin** `BUILDX_VERSION=0.38.1` and the two SHA-256 values in `fetch-buildx.sh`; fail on mismatch or missing slice.
- **Correction to the earlier doc:** the Darwin and Windows digests are **not** in the release's `checksums.txt`
  (it lists Linux/BSD only). They are in `checksums-signed.txt` and in GitHub's asset digests; both agree with the pins.
  Cross-check against both, not `checksums.txt`.
- **Upstream signature:** the Mach-O files are Developer-ID-signed by Docker Inc. with hardened runtime. Harpoon
  re-signs with its own Developer ID (`--options runtime --timestamp`, like the Docker CLI block in `sign-app.sh`);
  `spctl` reports "not an app" for a bare binary, which is expected and not a notarization failure (it is covered by
  the enclosing app's notarization).
- **Provenance:** `gh attestation verify` finds no GitHub attestation (404). The release ships SLSA provenance JSON,
  but its subject digest (`fd4e7d08…`) is of the **pre-signing** binary and cannot match the signed file. So
  provenance *content* is available but is not a verification path for the shipped bytes; the checksum pin plus
  Docker's code signature are. Treat as **Verified: checksum + Developer ID signature; not verified: SLSA chain**.
- **License:** Apache-2.0 notice to be added to the bundle's notices.

## Release-size implications
arm64 +65.5 MB (uncompressed binary; the compressed DMG delta is to be measured with a real bundle);
universal DMG +136 MB on top of 288 MB. No effect on RC.2, which is untouched.

## Recommended implementation (unchanged from the compat doc, A6, with these amendments)
1. `harpoon/fetch-buildx.sh` per A6, cross-checking `checksums-signed.txt` **and** the pins.
2. Bundle as `Resources/harpoon/bin/docker-buildx`; sign like the Docker CLI; extend `verify-bundle.sh`/`verify-signatures.sh`.
3. The engine invokes **only** the bundled binary, with an explicit environment (`DOCKER_HOST` from Harpoon's socket
   path, empty `HOME`/`DOCKER_CONFIG` under Harpoon's own directory). Missing/non-executable ⇒ the build step fails
   with a clear message; no fallback to `docker build`.
4. Engine builds never emit a `# syntax=` directive and reject Dockerfiles that request an external frontend unless the
   registry is reachable (see gate G1).
5. Add `tools/verify-buildx-build.sh` (A7 #5) using the exact tests above, including both negative controls.

## Open compatibility gates
| Gate | Finding | Needed before |
|---|---|---|
| **G1 — guest registry DNS** | A Dockerfile with `# syntax=docker/dockerfile:1` failed: BuildKit's resolver queried `192.168.64.1:53` ("connection refused"), while containers resolve via `1.1.1.1`. The cause (daemon-side resolver vs. host DNS forwarding) is **not diagnosed**; the host itself had network. Consequence: any `FROM` image **not already in the guest** (and any external frontend) may fail to resolve. | First real deployment must use a base image already present locally, or the cause is diagnosed first. Do not change guest assets to work around it without a decision. |
| G2 — secrets and the build cache | Cache key excludes secret content (see above). | Milestone C. |
| G3 — Intel guest | amd64 binary runs under Rosetta; never exercised against an x86_64 guest. | Before promoting the universal DMG beyond experimental. |
| G4 — clean-Mac gate | No Mac without Docker Desktop was available; absence was simulated by empty `HOME`/`DOCKER_CONFIG`/`PATH`. | Run test 5 (A7) on a second Mac before shipping the bundle. |
| G5 — SLSA chain | Not verifiable for signed Mach-O (see above). | Accept the risk explicitly or add reproducible-build verification later. |
| G6 — `docker buildx` in the user's Terminal | Out of scope (touches user config). | Separate decision. |
