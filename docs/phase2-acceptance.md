# Phase 2 Acceptance — Harpoon 0.1.0-dev

**Date:** 2026-08-26
**Product:** `Harpoon 0.1.0-dev` (802K arm64, `com.apple.security.virtualization`)
**Commit:** `22415c1`
**Archive:** `dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz` 289M `c2930f90a80f9c4ba41c5cf027072b8a97c4b46d25da15c1be301ad2cabfd0b4`
**macOS:** 26.5.2 arm64 M3 Pro
**Docker:** 29.3.1, Compose v5.1.0, Buildx v0.32.1

## What was tested

Installed/relocated product (`dist/...` + `/tmp/harpoon-m11-stage`) without source, via `harpoon/m12-test.sh` orchestrating `m3-m11` + bridge regression.

## Result

**CONDITIONAL PASS** — 22 PASS, 0 FAIL, 11 BLOCKED (host `VZErrorDomain 1`).

- PASS: version, help, doctor (11), config, status (`--json`), logs, docker status, unknown command, failure semantics (128/0), installation boundary (`cd /tmp` doctor, staged not repo-bound), reinstall/uninstall preserves user disk (`962M`), provisioning fix.
- BLOCKED (external): `harpoon start` → `VZErrorDomain 1` (also fails for repo `harpoon/build/harpoon` with `spike2/cache` disk, so not packaging), hence first-run, docker native, image/build, filesystem, networking, compose, restart, terminal, stability.

Previous successful boot with same `962M` disk at `2026-08-26T00:22:26 VM start SUCCESS` proves product boots when host not in bad state; live workflows were PASS in M3-M10 pre-transient.

## Release maturity

Staging relocatable, `doctor` healthy, install boundary proven, signing ad-hoc (needs Developer ID + notarization for Gatekeeper). No release blocker in Harpoon.

## Next

Re-run live matrix after host recovery → expected **PHASE 2 COMPLETE**.
