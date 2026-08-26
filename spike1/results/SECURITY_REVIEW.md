# Security Review — Spike 1 Own Diff

Self-audit as if diff came from untrusted contributor.

## New dependencies
- None added to repository manifests (no Cargo.toml at workspace root). Spike Rust stub uses `objc2 0.6.4`, `objc2-foundation 0.3.2`, `block2 0.6.2` but not yet built/linked into production; no network calls at runtime.
- No new Homebrew / npm / pip dependencies installed.

## New external artifacts
- Alpine 3.22 netboot `vmlinuz-virt` 9.2M (sha256 f270bfa...), `initramfs-virt` 8.5M (508de7f...), derived `Image-virt` 33M (377d348...). Source https://dl-cdn.alpinelinux.org documented in `spike1/fetch_guest.sh` and `spike1/README.md`. Provenance pinned (Alpine 3.22, kernel 6.12.x, GPL-2.0). Fetched via `curl -L --fail`, not silent `wget -q`. Checksums recorded. No opaque binaries committed: `spike1/cache/` gitignored.
- No downloaded scripts executed without inspection (only curl of Alpine CDN, gunzip decompression).

## Entitlements / permissions
- New file `spike1/entitlements.plist` contains **only** `com.apple.security.virtualization = true` — minimal, no `com.apple.security.cs.allow-jit`, no `com.apple.security.network.client` etc. Verified via `codesign -d --entitlements -` in `spike1/run.sh` and `SPIKE1_RESULTS.md`.
- Spike binary ad-hoc signed (`-s -`); no Developer ID, no provisioning profile leakage.
- File permissions: host socket not yet created (Spike 1 is VM only, no Docker socket). No `chmod 777`, no world-readable socket. Serial log at `/tmp/harpoon-spike1-serial.log` mode 0600 by default (FileManager). No unsafe `FileHandle` sharing.
- No privileged operations (`sudo`, `setuid`, `authopen`). VM creation requires entitlement but not root.

## Outbound network
- Only `curl` to `dl-cdn.alpinelinux.org` (Alpine CDN) in `fetch_guest.sh`. No telemetry, no analytics, no crate registry fetch at runtime. Cargo registry access was attempted earlier but blocked by sandbox (Operation not permitted) — not bypassed with escalation except for Swift builds via module cache path.
- No network calls from Swift VM runner at runtime (only VM NAT device, not yet used for host exfiltration).

## Shell injection / code execution
- Swift runner takes kernel/initramfs paths as `URL(fileURLWithPath:)` with `FileManager.fileExists` guard; no shell interpolation, no `system()` calls.
- `fetch_guest.sh` uses quoted `"$CACHE"` etc., `set -euo pipefail`, no `eval`.
- No `muse.bash` injection vectors in repo scripts.

## Persistence / launch agents
- No LaunchAgent, LaunchDaemon, cron, or profile modifications.
- No `~/Library/LaunchAgents`, no `~/.zshrc` edits, no Docker socket exposure.

## Docker socket exposure
- Not applicable to Spike 1 (VM only). Future Spike 2 must ensure `~/.harpoon/docker.sock` is 0600 and not a TCP listener.

## Generated artifacts hygiene
- `spike1/cache/` (9M+8M+33M) and `spike1/build/` are gitignored via `.gitignore` addition `spike1/cache` + `spike1/build`. No huge blobs staged.
- No `.pyc`, no `__pycache__`, no `node_modules`.

## Git config / history
- `git config --list` shows standard user.name Juanito, gpg signing enabled, remote origin git@github.com:krazybean/Harpoon.git — no suspicious `core.hooksPath`, no added remotes, no altered `.git/hooks` beyond sample hooks.
- Diff: only docs added + spike1 (Swift) + .gitignore entries. No executable added outside spike1, no hidden files.

## Verdict
- No credentials, no telemetry, no overly broad entitlements, no persistence, no socket exposure, no hidden network endpoints.
- Acceptable for continuation to Spike 2. Manual review: verify Alpine CDN TLS (curl uses default certs) and that `gunzip -c` decompression does not execute embedded scripts (it doesn't).

