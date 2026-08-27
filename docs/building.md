# Building Harpoon

This document is the canonical guide for building Harpoon from source and producing the self-contained macOS distribution. It describes what is compiled on the build machine and what an end user receives.

## 1. Overview

```
Development / build machine                End-user Mac
--------------------------                 ------------
Node.js 20                               (not required)
npm                                      (not required)
Rust / Cargo                             (not required)
Tauri CLI (local)                        (not required)
Swift / Xcode tools                      Virtualization.framework (system)
        |                                         |
        v                                         v
Harpoon runtime  ─┐                                |
Tauri frontend   ─┤─► Tauri bundle ─► Harpoon.app / DMG ──► double-click / drag to /Applications
        |         │
        v         v
   Harpoon.app / DMG is self-contained; it carries the desktop UI, the Harpoon runtime, and the Linux guest artifacts. Writable runtime state is provisioned outside the read-only app bundle.
```

End-user requirements after packaging:

- does **not** require Node.js
- does **not** require npm
- does **not** require Rust
- does **not** require Cargo
- does **not** require the Tauri CLI
- does **not** require a source checkout

The desktop build uses Tauri's normal bundling facilities; no custom installer daemon, updater, or launch agent is involved.

## Quick Start

**Development:**

```sh
cd ui/harpoon-desktop
npm ci
npm run tauri dev
```

**Release build (no version bump):**

```sh
npm run build:release
# or from ui/harpoon-desktop:
# npm run build:release
# which builds runtime → frontend → bundle-resources → Harpoon.app → signs → verifies → DMG (3072 MiB)
```

**Bump patch + release:**

```sh
npm run release -- patch
```

**Other bumps/releases:**

```sh
npm run release -- minor   # 0.1.0 -> 0.2.0
npm run release -- major   # 0.1.0 -> 1.0.0
npm run release -- 0.2.0   # explicit
```

**Version only (no build):**

```sh
npm run version:bump -- patch
npm run version:bump -- 0.2.0
```

**Verify versions:**

```sh
npm run version:check
```

**Cleanup (safe, removes only rebuildable artifacts):**

```sh
npm run clean:release
# or: npm run clean:release -- --dry-run   # preview
```

### Semantic Versioning

Harpoon uses `MAJOR.MINOR.PATCH` per SemVer:

- `PATCH` — bug fixes, internal improvements, no intentional compatibility break (e.g., `0.1.0 -> 0.1.1`)
- `MINOR` — backwards-compatible features (e.g., `0.1.0 -> 0.2.0`)
- `MAJOR` — breaking compatibility / release boundary (e.g., `0.1.0 -> 1.0.0`)

Prerelease syntax is supported as `MAJOR.MINOR.PATCH-prerelease` (e.g., `1.0.0-beta.1`, `1.0.0-alpha.0`). Use:

```sh
npm run version:bump -- prerelease   # 0.1.0 -> 0.1.1-0, 1.0.0-beta.1 -> 1.0.0-beta.2
npm run version:bump -- 1.0.0-beta.1 # explicit prerelease
```

Version bump does **not** automatically `git commit`, `git tag`, or `git push`. Commit and tag separately after verifying `npm run version:check`.

### DMG Size Note

Harpoon bundles a sparse 2 GiB Linux root filesystem (`spike2/cache/harpoon-root.img`). APFS reports a much smaller physical footprint than the logical space required when copied into the temporary HFS+ DMG. The release workflow therefore creates the DMG with an explicit working-image size:

```sh
--disk-image-size 3072
```

You do not need to invoke this manually — `npm run build:release` / `npm run release -- patch` handles it via `scripts/release.mjs` → `bundle_dmg.sh --disk-image-size 3072` (fallback `hdiutil create -size 3072m`). The final compressed v0.1 DMG is approximately 292 MiB.

## 2. Platform requirements

Harpoon uses Apple's `Virtualization.framework`, so building and running the VM requires **macOS on Apple Silicon (arm64)**. The current project targets Apple Silicon hosts and ARM64 Linux guests (see `docs/compatibility.md` and `docs/architecture.md`). No Intel or cross-platform claim is made.

Build machine also needs the normal macOS command-line development tools for Swift, Rust, and Tauri:

- Xcode Command Line Tools (`xcode-select --install`) — provides `swiftc`, `codesign`, and the macOS SDK
- Rust toolchain (`rustup` stable `aarch64-apple-darwin`) — for the Tauri desktop

No minimum macOS version is invented here; the repository does not declare one beyond what `Virtualization.framework` and the current `tauri.conf.json` imply.

## 3. Node / NVM setup

The desktop UI (`ui/harpoon-desktop`) expects **Node 20**. `ui/harpoon-desktop/package.json` declares the frontend toolchain and pins `@tauri-apps/cli` as a local `devDependency`.

On a shell where NVM is not automatically sourced, use:

```sh
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
nvm use 20

node --version
npm --version
```

Expected (example):

```
v20.x.x
10.x.x
```

Troubleshooting: if the wrong system Node is active, `npm` may fail with:

```
Error: Cannot find module '../lib/cli.js'
```

Fix by sourcing NVM as above, running `nvm use 20`, and verifying `which node` and `which npm` point inside `$NVM_DIR/versions/node/v20*/bin`.

## 4. Install development dependencies

From `ui/harpoon-desktop` install the reproducible frontend dependencies:

```sh
npm ci
```

Use `npm ci` when `package-lock.json` exists (it does at `ui/harpoon-desktop/package-lock.json`). `npm ci` is reproducible from the committed lockfile.

The Tauri CLI comes from the project's local dependencies (`@tauri-apps/cli` in `devDependencies`). After `npm ci`, invoke it via the npm script or the local binary:

```sh
npm run tauri -- --version
# or
./node_modules/.bin/tauri --version
```

Do **not** install `@tauri-apps/cli` globally. The repository does not require it.

The Rust side is reproducible via `ui/harpoon-desktop/src-tauri/Cargo.lock`. `cargo build` / `cargo check` respect the lockfile; no extra step is needed.

### Version source of truth

`ui/harpoon-desktop/package.json` `version` is canonical. `src-tauri/tauri.conf.json` `version` and `src-tauri/Cargo.toml` `version` are synchronized from it via `npm run version:bump` / `scripts/version.mjs`. Verify consistency with `npm run version:check`. Do not edit those versions manually.

## 5. Run the UI in development

```sh
cd ui/harpoon-desktop
npm run tauri dev
```

This starts Vite (`npm run dev` at `http://localhost:1420` per `tauri.conf.json`) and launches the Tauri window. The UI discovers the Harpoon runtime via its binary resolver (`HARPOON_BIN` → bundled `Harpoon.app` → `harpoon/build/harpoon` → `/usr/local/bin/harpoon` → `PATH`) and follows the current bootstrap state machine: on first launch it will automatically attempt `harpoon start` once and show phased progress (`Checking runtime…` → `Starting Harpoon…` → `Starting Linux VM…` → `Waiting for Docker Engine…` → `Ready`). No manual `harpoon start` is required in development, though the CLI remains available.

## 6. Build Harpoon runtime

From repository root, the canonical runtime build is:

```sh
bash harpoon/build.sh
```

This compiles the Swift runtime (`harpoon/Sources/*.swift` with `-framework Virtualization`) to `harpoon/build/harpoon` and ad-hoc signs it with `harpoon/entitlements.plist` (`com.apple.security.virtualization`). Output is a single `Mach-O arm64` executable; `harpoon doctor` validates it.

## 7. Build frontend

Verified frontend build (from repository root):

```sh
npm --prefix ui/harpoon-desktop ci
npm --prefix ui/harpoon-desktop run build
```

This runs `tsc && vite build` (`ui/harpoon-desktop/package.json` `scripts.build`) and emits `ui/harpoon-desktop/dist/` (`index.html` + `assets/`). The same command is used by Tauri's `beforeBuildCommand`.

## 8. Prepare bundled Harpoon resources

The macOS application bundles the Harpoon executable and its required Linux guest artifacts for inclusion in `Harpoon.app`. The current helper is:

```sh
bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh
```

It stages:

- `harpoon/build/harpoon` → `ui/harpoon-desktop/src-tauri/bundle-resources/harpoon/bin/harpoon`
- `spike1/cache/Image-virt` → `.../lib/harpoon/Image-virt`
- `harpoon/cache/harpoon-m4-initramfs.cpio.gz` → `.../lib/harpoon/harpoon-initramfs.cpio.gz`
- `spike2/cache/harpoon-root.img` → `.../lib/harpoon/harpoon-root.img` (APFS clone-aware via `cp -c` → `ditto` → `cp`)

In the current `src-tauri/build.rs`, this staging is also performed automatically during `cargo build`, so an explicit `prepare-bundle.sh` run is not strictly required before `tauri build` — it is useful to inspect `bundle-resources/` or to prepare without invoking Cargo. Do not treat `prepare-bundle.sh` as a long-term public API; `build.rs` is the source of truth for what is staged.

Generated `bundle-resources/` is ignored by `.gitignore` and must not be committed.

## 9. Build packaged application

Verified Tauri release build (from repository root):

```sh
npm --prefix ui/harpoon-desktop run tauri -- build --bundles app
```

This:

1. Runs `beforeBuildCommand` (`npm run build`) to refresh `ui/harpoon-desktop/dist/`.
2. Invokes `src-tauri/build.rs` to ensure `bundle-resources/harpoon` is populated.
3. Compiles the Tauri Rust desktop (`src-tauri/src/main.rs`) in release mode.
4. Bundles `Harpoon.app` using Tauri's bundler.

Artifacts (`Tauri v2` default layout):

```
ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app
ui/harpoon-desktop/src-tauri/target/release/bundle/dmg/   # when supported
```

The project currently sets `tauri.conf.json`:

```json
"bundle": {
  "active": true,
  "targets": ["app", "dmg"],
  "resources": {"bundle-resources/harpoon": "harpoon"},
  "macOS": { "signingIdentity": null, "entitlements": null }
}
```

- **Development build:** `npm run tauri dev` (no bundle, Vite dev server).
- **`.app` bundle:** the self-contained application (always produced when `bundle.active` is true).
- **DMG / distribution artifact:** produced when `dmg` target is supported by the installed Tauri/bundler and macOS tooling; if `hdiutil`/`dmg` support is unavailable, the `.app` alone satisfies D1. Check `target/release/bundle/dmg/` after build.

To build a DMG explicitly when configured:

```sh
npm --prefix ui/harpoon-desktop run tauri -- build --bundles dmg
# or both
npm --prefix ui/harpoon-desktop run tauri -- build --bundles app,dmg
```

## 10. What gets bundled

Conceptually, `Harpoon.app` contains:

```
Harpoon.app/Contents/
  MacOS/harpoon-desktop          # desktop UI executable (Tauri)
  Resources/harpoon/
    bin/harpoon                  # Harpoon runtime executable
    lib/harpoon/
      Image-virt                 # ARM64 Linux kernel
      harpoon-initramfs.cpio.gz  # initramfs
      harpoon-root.img           # root-disk template (sparse, read-only inside bundle)
```

The canonical layout is `Resources/harpoon/...` as above. `tauri.conf.json` now uses a resource map (`"bundle-resources/harpoon": "harpoon"`) so Tauri copies the staging directory `bundle-resources/harpoon` to `Resources/harpoon` without duplicating the `bundle-resources` prefix. The resolver retains a legacy fallback probe for `Resources/bundle-resources/harpoon` for compatibility with older builds, but the shipped artifact contains exactly one `Resources/harpoon` tree.

Writable runtime state is **not** inside the app bundle. On first run, `harpoon/Sources/RuntimeConfig.swift` provisions a persistent, writable disk outside the bundle:

- `~/Library/Application Support/Harpoon/data/harpoon-root.img` (preferred)
- fallback `/tmp/harpoon-runtime/data/harpoon-root.img`

The template inside `Harpoon.app` is treated as read-only; the copy is created with APFS clone (`cp -c`) where available. Upgrading `Harpoon.app` (dragging a new `.app` over the old) does not destroy the existing `~/Library/Application Support/Harpoon/` directory — images, volumes, and `config.json` are preserved.

## 11. Verify packaged application

For a copied test application such as `/tmp/Harpoon.app`:

```sh
# optional: ensure the app's bundled runtime is used, not a dev checkout
env -u HARPOON_BIN

# the app's Harpoon is at:
ls /tmp/Harpoon.app/Contents/Resources/harpoon/bin/harpoon
/tmp/Harpoon.app/Contents/Resources/harpoon/bin/harpoon doctor
/tmp/Harpoon.app/Contents/Resources/harpoon/bin/harpoon status --json
```

Start Harpoon (via the app's UI auto-start or via the bundled binary):

```sh
/tmp/Harpoon.app/Contents/Resources/harpoon/bin/harpoon start
/tmp/Harpoon.app/Contents/Resources/harpoon/bin/harpoon status
```

Wait for `Harpoon: running` / `status --json` `state: "running"` `dockerReady: true`, then verify through Harpoon's Docker socket:

```sh
export DOCKER_HOST=unix:///tmp/harpoon-docker.sock

docker version
docker info
docker run --rm alpine:3.22 true
```

Success means:

- `harpoon status` shows VM `running`
- `dockerReady` is true / `harpoon doctor` shows `Docker API reachable`
- `docker version` `Server:` is `Linux/arm64` inside Harpoon, not `Docker Desktop`
- `docker info` `Server Version` matches the guest Engine
- `alpine:3.22 true` exits `0`

Note: `docker info` may list host-installed Docker CLI plugins (e.g., `buildx`, `compose`). Their presence reflects the **client** environment, not the Engine; the `Server:` section is authoritative for the Engine being reached. This does not mean Docker Desktop is the Engine.

If the host is in a transient `HOST_VZ_START_FAILURE` window, `harpoon start` will report `VZErrorDomain 1` and `status` will remain `stale`; this is a host Virtualization.framework condition (see Troubleshooting), not a packaging failure.

## 12. Docker context alternative

Harpoon also exposes the same socket via a Docker context:

```sh
docker --context harpoon version
docker --context harpoon ps
docker --context harpoon info
```

The context `harpoon` points at `unix:///tmp/harpoon-docker.sock` (`harpoon docker setup` creates it). Using `--context harpoon` is equivalent to `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` and avoids silently falling back to the `default`/`desktop-linux` context that may point at Docker Desktop. For verification, prefer the explicit `--context harpoon` or `DOCKER_HOST` form and confirm the `Server` is Harpoon's.

## 13. Troubleshooting

### A. Wrong Node version / broken npm

Symptom:

```
Error: Cannot find module '../lib/cli.js'
```

Cause: system Node (often an old `/usr/local/bin/npm`) is active instead of NVM's Node 20.

Fix:

```sh
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
nvm use 20

which node   # should be $NVM_DIR/versions/node/v20*/bin/node
which npm    # should be $NVM_DIR/versions/node/v20*/bin/npm
node --version
npm --version
```

Then re-run `npm ci` in `ui/harpoon-desktop`.

### B. `tauri: command not found`

After `npm ci`, the `tauri` command is available locally:

```sh
npm run tauri -- --version
# or
./ui/harpoon-desktop/node_modules/.bin/tauri --version
```

The `package.json` `scripts.tauri: "tauri"` resolves to the local `node_modules/.bin/tauri`. Do **not** `npm install -g @tauri-apps/cli` as the default fix; use the project's local `devDependency` (`@tauri-apps/cli`).

### C. HOST_VZ_START_FAILURE / VZErrorDomain 1

Harpoon may report:

```
VZErrorDomain 1
Internal Virtualization error.
The virtual machine failed to start.
```

Or via status/logs:

```
HARPOON_STATE BOOTING -> FAILED reason=VM start failure VZErrorDomain 1
HOST_VZ_START_FAILURE
```

This is an observed, intermittent host `Virtualization.framework` startup failure that has been characterized separately from packaging (see `docs/results/R1.md`, `docs/results/D1.md`, and `harpoon/results/r1/`). It can occur with the same Harpoon binary in both repository (`harpoon/build/harpoon`) and bundled (`Harpoon.app/Contents/Resources/harpoon/bin/harpoon`) execution.

Do not treat it as a Harpoon packaging bug, and do not assume it has been fixed. The UI surfaces it as `Harpoon could not start` with `Phase: failed` and a `Retry` action; one automatic start is attempted on launch, further retries are manual and bounded.

For deeper diagnostics use `harpoon doctor`, `harpoon status --json`, and `harpoon logs --lines`.

## 14. Signing and distribution status

What is true now:

- Local/ad-hoc application signing has been exercised (`codesign --sign -` via Tauri, `harpoon/build/harpoon` ad-hoc with `harpoon/entitlements.plist` `com.apple.security.virtualization`). `codesign -dv --verbose=4` shows `adhoc,linker-signed` and `codesign --verify --deep --strict` passes for `Harpoon.app`.
- The Harpoon runtime **requires** the virtualization entitlement (`com.apple.security.virtualization`); the Tauri desktop executable does **not** receive it.
- Packaged application execution has been proven (`Harpoon.app` launches, `harpoon doctor` from bundle shows `binary .../Resources/harpoon/bin/harpoon` and `kernel .../Image-virt` at bundle path).
- Self-contained packaged runtime has been proven (the app carries `harpoon`, `Image-virt`, `harpoon-initramfs.cpio.gz`, `harpoon-root.img`; writable state is outside the bundle).

What is **not** claimed:

- **Developer ID signing** and **Apple notarization** are **not** complete for public distribution. Local ad-hoc / `Apple Development` signing (any valid Apple Development identity if available) is sufficient for local acceptance, but public macOS distribution via Gatekeeper requires a `Developer ID Application` certificate and `xcrun notarytool` / `stapler` notarization, which is separate from building Harpoon from source.

Building Harpoon from source does not require Developer ID or notarization; those are distribution-time steps.

## 15. Clean build recipe

Concise copy/paste from repository root (verify paths exist before running):

```sh
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
nvm use 20

bash harpoon/build.sh

npm --prefix ui/harpoon-desktop ci
npm --prefix ui/harpoon-desktop run build

bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh

npm --prefix ui/harpoon-desktop run tauri -- build --bundles app
```

Notes:

- `bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh` is idempotent and can be omitted when building via `tauri build`, because `ui/harpoon-desktop/src-tauri/build.rs` also stages `bundle-resources/harpoon` automatically. It is documented here for explicit preparation or inspection (`ls src-tauri/bundle-resources/harpoon/{bin,lib/harpoon}`) and is harmless to run twice.
- For a DMG when supported, replace the last line with `npm --prefix ui/harpoon-desktop run tauri -- build --bundles dmg` or `app,dmg`.
- Resulting artifacts: `ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app` (and `.../dmg/` when built).
