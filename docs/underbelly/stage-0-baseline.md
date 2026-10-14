# Underbelly Stage 0 — Baseline Freeze

**Captured:** 2026-10-13 (America/Chicago)
**Scope:** repository and documented runtime baseline only. No runtime or behavior changes were made.

## Repository identity

| Item | Observation |
|---|---|
| Repository | `github.com/krazybean/Harpoon` |
| Branch | `main` |
| HEAD | `642d91dc7ea8fdbfa4ca9206391ce38ede275e6a` (`642d91d`) |
| HEAD date | `2026-10-11T23:47:32-05:00` |
| Upstream relation | `main...origin/main` before this phase |
| Working tree | Clean before this phase; only the three Stage 0–1 documents in this change are new |

## Tool and runtime observations

These are commands run on the current Apple Silicon host, not universal project requirements.

| Tool / layer | Observed now | Repository/documented reference |
|---|---|---|
| macOS | 27.0.1, arm64 | README resource evidence: 26.5.2 arm64 M3 Pro |
| Swift | Apple Swift 6.4.0.34.1 | Swift + `Virtualization.framework` build in `harpoon/build.sh` |
| Rust | `rustc 1.97.1`, `cargo 1.97.1` | Rust stable required by `README.md` / `docs/building.md` |
| Node | v22.23.3 | `tools/validate.sh` requires Node 20; this is a baseline mismatch |
| npm | 10.9.9 | `ui/harpoon-desktop/package.json` / lockfile |
| Docker client / Engine | 29.9.0 / 28.3.3 | README and results commonly record Engine 28.3.3; historical acceptance docs record Docker 29.3.1 |
| Compose | v5.1.0 | README / acceptance references |
| Buildx | v0.32.1-desktop.1 | README / acceptance references |
| Guest | Alpine 3.22, ARM64, kernel family 6.12.x | `tools/guest-builder/README.md` |
| Guest runtime | `dockerd`, `containerd`, `runc`, `socat`, Python management service | `tools/guest-builder/src/init`, runtime-closure verifier, and guest sources |
| Minimum macOS | `15.1` build target | `harpoon/MIN_MACOS`, `harpoon/build.sh`; `docs/compatibility.md` still says 13+ and `docs/building.md` says no minimum is declared |
| UI/runtime version | UI/Tauri metadata is `0.1.1`; Swift CLI and legacy packaging still identify `0.1.0` | `ui/harpoon-desktop/package.json`, `harpoon/Sources/HarpoonCLI.swift`, `harpoon/package.sh` |

## Current acceptance references

The repository has several layers of acceptance evidence. They are not interchangeable:

- Fast repository gate: `sh tools/validate.sh`.
- Guest/runtime closure: `tools/release/verify-runtime-closure.sh`, `tools/guest-builder/verify-guest.sh`, and `harpoon/regression-*` scripts.
- Lifecycle and Docker compatibility harnesses: `harpoon/m3-test.sh` through `harpoon/m18-test.sh`, `harpoon/ec-test.sh`, `harpoon/ui-test.sh`, and `harpoon/cli-parity-test.sh`.
- Scope and acceptance contracts: `docs/requirements.md`, `docs/phase2-acceptance.md`, `docs/compatibility.md`, and `docs/docker-integration.md`.
- Historical evidence: `docs/results/` and preserved `harpoon/results/` material. Results marked `BLOCKED` or `CONDITIONAL PASS` remain qualified evidence, not fresh green runs.

## Baseline checklist

| Check | Result | Evidence / qualification |
|---|---|---|
| Git identity and clean main | PASS | HEAD and status recorded above |
| Runtime ownership invariant | PASS | README, SECURITY.md, requirements, and current sources agree that Docker owns container/image/network/volume/API semantics and Harpoon owns the macOS↔Linux boundary |
| Repository validation gate | BLOCKED | `sh tools/validate.sh` exited 2: `requires Node 20; found v22.23.3` |
| Live VM boot | NOT RUN | No behavior change or live acceptance run was requested for Stages 0–1 |
| Docker Engine end-to-end acceptance | NOT RUN | Historical evidence exists; no fresh VM session was claimed |
| Guest artifact closure | NOT RUN | Structural scripts were inspected, not re-run |
| Current host/tool drift captured | PASS | Current versions differ from several historical result documents |
| Roadmap freshness | STALE | `docs/roadmap.md` still identifies commit `22415c1` / an older dirty-tree snapshot rather than HEAD `642d91d` |
| Version metadata consistency | STALE | `0.1.1` UI/release metadata coexists with `0.1.0` Swift CLI and legacy packaging paths |
| Canonical anatomy captured | PASS | See `docs/architecture/runtime-anatomy.md` |
| Stale documentation flagged | PASS | See the drift section in the anatomy document |

## Freeze decisions

1. No production source, guest image, acceptance harness, or runtime behavior was changed.
2. Docker Engine remains authoritative for containers, images, networks, volumes, Compose, BuildKit, Docker API behavior, and OCI/containerd/runc semantics.
3. Harpoon remains authoritative for the macOS↔Linux boundary: VM lifecycle, Virtualization.framework configuration, host sockets, vsock transport, VirtioFS, published-port translation, guest readiness, and boundary diagnostics.
4. Historical claims are retained as evidence with their dates and qualifications; they are not silently promoted to current-host facts.
