# Milestone B2 — bundled Docker Buildx packaging

Status: implemented on `feature/buildx-packaging`; **not merged, not released**. Follows
`DEPLOY_BUILDX_VALIDATION.md` (pins, signature, gates G1–G6). Date: 2026-10-30.

## What ships
`Harpoon.app/Contents/Resources/harpoon/bin/docker-buildx` — upstream Docker Buildx **v0.38.1**, unmodified, plus
`share/licenses/docker-buildx/{LICENSE,NOTICE.md}` (Apache-2.0; text from upstream tag `v0.38.1`). Nothing is committed
as a binary; `harpoon/build/` is git-ignored.

## Pipeline
| Step | Where | Fails closed on |
|---|---|---|
| Fetch + verify | `harpoon/fetch-buildx.sh` (`HARPOON_ARCH=arm64` default, or `universal`) | SHA-256 ≠ in-script pin; digest ≠ `checksums-signed.txt` (or entry missing); upstream signature not valid/strict, not `Developer ID Application: Docker Inc (9BNSXJN65R)`, no hardened runtime, no timestamp; wrong Mach-O architecture; missing slice; unsupported arch. A failed run deletes any previous output. |
| Stage | `prepare-bundle.sh` (and `build.rs` for plain `tauri build`) | release builds require it |
| Sign | ad-hoc: `ui/harpoon-desktop/scripts/sign-app.sh`; Developer ID: `tools/release/macos/sign-app.sh` (`--options runtime --timestamp`, before the outer app) | bundle without `docker-buildx` is not signed |
| Verify | `tools/check-bundled-buildx.sh`, called by `verify-bundle.sh` (pre-release) and `verify-signatures.sh` (`--release`) | missing/symlink/non-executable; architecture set ≠ exactly `arm64` (or `arm64 x86_64` for universal); invalid signature; Developer ID without hardened runtime/timestamp; release: ad-hoc, not Developer ID, or TeamIdentifier ≠ Harpoon.app's (i.e. still Docker Inc.'s); missing license; `version` (run with `env -i`, empty `HOME`/`DOCKER_CONFIG`, no daemon) not `v0.38.1` |
| Resolve at run time | `deploy/src/buildx.rs` (pure, fake-filesystem tested) | missing / not a file / symlink / not executable / unknown Resources dir → actionable error that names the file, how to fix it, and states there is **no fallback** to Docker Desktop, Homebrew, a user-installed buildx or the legacy builder. Exactly one path is probed. `isolated_env` yields the only environment a buildx process may get (`PATH`, `HOME`, `DOCKER_CONFIG`, `DOCKER_HOST`). |

Upstream's signature is checked **before** repackaging and is then replaced by Harpoon's, as for the Docker CLI.
`lipo` of the two verified slices keeps a valid signature (verified); arm64 output carries the same CDHash as upstream.

## Evidence
- `harpoon/regression-buildx-packaging.sh`: 41 checks, 0 skipped (wrong checksum, tampered cache, signed-sums mismatch /
  missing entry, offline without sums, missing x86_64 slice, bad `HARPOON_ARCH`, exact architectures, universal signature,
  foreign/unsigned/tampered signature, missing/symlinked/non-executable/wrong-arch/ad-hoc/unsigned/no-license bundles,
  signing scripts refusing a bundle without Buildx, no binary tracked in git).
- Ad-hoc workflow: `ui/.../sign-app.sh` on a throwaway app tree, then pre-release check: pass.
- Developer ID workflow: `tools/release/macos/sign-app.sh` with the local *Developer ID Application* identity on a throwaway
  app tree (never a release bundle, no notarization, nothing published): `docker-buildx` re-signed with hardened runtime and
  a secure timestamp, TeamIdentifier equal to the app's; `--release` check: pass.
- `deploy` crate: 118 tests (112 + 6 for `buildx.rs`); fmt and clippy `-D warnings` clean.
- `tools/verify-buildx-build.sh` is written but **was not run against a VM**. Only `--plan`, the refusal without
  `HARPOON_LIVE_VM_TESTS=1`, and `bash -n` were exercised.

## Size
| | raw | compressed (UDZO, estimate) |
|---|---|---|
| arm64 | +65.5 MB (65,495,984 B) | ≈ +25.0 MB |
| universal (arm64 + x86_64) | +136.1 MB (136,094,640 B) | ≈ +52.2 MB |
The compressed figures are `hdiutil` UDZO images of the binary alone; a real DMG delta must be measured at release time.

## Remaining gates
G1 guest registry DNS (undiagnosed) · G2 secrets vs cache key (Milestone C) · G3 Intel guest · G4 clean-Mac run of
`verify-buildx-build.sh` (needs a Mac without Docker Desktop and written authorization to build in a VM) · G5 SLSA chain
(unverifiable for the signed Mach-O) · G6 `docker buildx` in the user's Terminal (out of scope). Notarization of a bundle
containing the re-signed binary is untested here. The universal DMG path was exercised only up to the binary, not a full build.
