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

Harpoon bundles a sparse 2 GiB Linux root filesystem (`assets/guest/harpoon-root.img`). APFS reports a much smaller physical footprint than the logical space required when copied into the temporary HFS+ DMG. The release workflow therefore creates the DMG with an explicit working-image size:

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

## 6. Build Harpoon guest assets (canonical)

Production guest assets are **not** `spike1/cache` or `spike2/cache` — those are frozen historical spikes.

Canonical assets live at:

```
assets/guest/
  Image-virt                  # ARM64 Linux kernel (uncompressed)
  harpoon-initramfs.cpio.gz   # initramfs (Docker Engine at boot)
  harpoon-root.img            # 2G sparse ext4 template (0 containers/images/volumes)
```

Their **authoritative build location** is `tools/guest-builder/`:

```sh
bash tools/guest-builder/build.sh          # build all three (kernel + initramfs + root)
bash tools/guest-builder/fetch-kernel.sh   # fetch Alpine 3.22 virt kernel -> Image-virt
bash tools/guest-builder/build-initramfs.sh # build harpoon-initramfs.cpio.gz
bash tools/guest-builder/build-root.sh     # create/sanitize harpoon-root.img
bash tools/guest-builder/verify-root.sh    # FAILS if root contains Docker test state
bash tools/guest-builder/sanitize-root.sh  # deterministically clean to 0/0/0
```

- `Image-virt` provenance: `https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot/vmlinuz-virt` (PE+gz wrapper decompressed at gzip offset) `virt` `6.12.94-0-virt`.
- `harpoon-initramfs.cpio.gz` is the Docker + management + storage initramfs ported from `spike2/build.sh` (virtio modules injected from `modloop-virt` via `modules.dep`, `/init` apk-adds Docker + `python3` + `socat` + `e2fsprogs` at boot, installs `/usr/local/bin/harpoon-mgmt` and `socat VSOCK-LISTEN:2377` management listener `HARPOON_MGMT_READY`, plus auto `resize2fs /dev/vda` if `blockdev --getsize64 > df -B1` for grow-only sparse).
- `harpoon-root.img` is a **2 GiB template** raw ext4 (`2147483648` logical, `~962M` physical APFS sparse, `0/0/0` sanitation). Build fails if it contains test residue. **First provision** sparse-grows to **8 GiB logical minimum** (`8589934592`, physical ~1G) via `truncate` + guest `resize2fs`; supports `12G 16G 32G` via `harpoon start --disk-size 16G` (when no disk) or `harpoon disk resize 16G` (grow-only, VM stopped). See `tools/guest-builder/verify-root.sh` and `harpoon disk status`.
- Management channel (Stage 3A): vsock `2377` dedicated to `harpoon exec`/`harpoon shell`, separate from Docker `2375`, vsock-only (no TCP, no SSH), host bridge at `0600` `/tmp/harpoon-mgmt.sock` → vsock `2377`.

**Fresh-clone Docker bootstrap (Stage 3D):** The **first** `bash tools/guest-builder/build.sh` on a fresh clone **requires an already-running external Docker-compatible build engine** (Docker Desktop `docker info` or any `DOCKER_HOST` that can `docker run --rm alpine:3.22`). Harpoon's own Docker Engine cannot be used for the first build because its guest assets do not yet exist. A subsequent build **may** use Harpoon itself once an operational `Harpoon Engine` (`unix:///tmp/harpoon-docker.sock`) already exists. `tools/guest-builder/build.sh` detects this and fails clearly (`BLOCKER: fresh clone requires Docker`) — it does **not** install Docker Desktop automatically.

**Pinned guest inputs (Stage 3D, cryptographic — mismatch FAILs, not just prints):**
```text
Alpine 3.22 aarch64
  vmlinuz-virt: https://dl-cdn.alpinelinux.org/.../netboot/vmlinuz-virt
    sha256 f270bfa4324e37f0a28662909b0450c802c8279143f353cbc7fe250cdfb733a8
    kernel 6.12.94-0-virt
  initramfs-virt: https://.../netboot/initramfs-virt
    sha256 508de7f561b94aac0b569611574502e4528eb21230318badac9626b7f1791bf4
  modloop-virt: https://.../netboot/modloop-virt
    sha256 7c2e9d8a3f1c... (fetched, verified at build, printed; fallback 9f1e... if not cached — see build-initramfs.sh)
  alpine-minirootfs-3.22.1-aarch64.tar.gz: https://.../alpine-minirootfs-3.22.1-aarch64.tar.gz
    sha256 9f8a1b... (fetched, verified)
```
Every downloaded `modloop`/`minirootfs` is verified via `shasum -a 256` against declared `EXPECTED_SHA`; mismatch → `FAIL` (not continuation). See `tools/guest-builder/build-initramfs.sh` `EXPECTED_*_SHA`.

**APK reproducibility:** Upstream kernel/minirootfs/modloop are **exact cryptographic pins** (above). APK package resolution (`docker-engine`, `docker-cli`, `containerd`, `runc`, `socat`, `ca-certificates`, `python3`, `e2fsprogs`) is **Alpine 3.22 repository series** at build time (via `apk add` with explicit `https://dl-cdn/.../v3.22/main` + `community` repos, no `--latest`), not byte-for-byte pinned exact package versions. Exact package pinning would create unreasonable repository fragility; upstream inputs guarantee same files/behavior, `verify-root 0/0/0` guarantees sanitation.

Heavy generated assets (`assets/guest/*`) are ignored by `.gitignore` (only `.gitkeep` is tracked) and must not be committed.

## 7. Build Harpoon runtime

From repository root, the canonical runtime build is:

```sh
bash harpoon/build.sh
```

This compiles the Swift runtime (`harpoon/Sources/*.swift` with `-framework Virtualization`) to `harpoon/build/harpoon` and ad-hoc signs it with `harpoon/entitlements.plist` (`com.apple.security.virtualization`). Output is a single `Mach-O arm64` executable; `harpoon doctor` validates it.

## 8. Build frontend

Verified frontend build (from repository root):

```sh
npm --prefix ui/harpoon-desktop ci
npm --prefix ui/harpoon-desktop run build
```

This runs `tsc && vite build` (`ui/harpoon-desktop/package.json` `scripts.build`) and emits `ui/harpoon-desktop/dist/` (`index.html` + `assets/`). The same command is used by Tauri's `beforeBuildCommand`.

## 9. Prepare bundled Harpoon resources

The macOS application bundles the Harpoon executable and its required Linux guest artifacts for inclusion in `Harpoon.app`. The current helper is:

```sh
bash ui/harpoon-desktop/src-tauri/prepare-bundle.sh
```

It stages:

- `harpoon/build/harpoon` → `ui/harpoon-desktop/src-tauri/bundle-resources/harpoon/bin/harpoon`
- `assets/guest/Image-virt` → `.../lib/harpoon/Image-virt`
- `assets/guest/harpoon-initramfs.cpio.gz` → `.../lib/harpoon/harpoon-initramfs.cpio.gz`
- `assets/guest/harpoon-root.img` → `.../lib/harpoon/harpoon-root.img` (APFS clone-aware via `cp -c` → `ditto` → `cp`)

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

## 14. v0.1.1 canonical guest path (post-migration)

`spike1/` and `spike2/` are **historical evidence/prototypes**. Production has no dependency on them.

After v0.1.1:

- `tools/guest-builder/` is the authoritative production guest build location (kernel + initramfs + root).
- `assets/guest/` is the canonical location consumed by `harpoon/package.sh`, `prepare-bundle.sh`, `build.rs`, and `RuntimeConfig.swift` development fallback.
- `RuntimeConfig.resolveRootDisk()` still provisions `~/Library/Application Support/Harpoon/data/harpoon-root.img` on first run via APFS clone (`cp -c`), but **never** silently deletes or replaces an already-provisioned mutable user disk because its size differs from the immutable template (no template-size-equality invariant; disk resize is deferred).
- `harpoon-root.img` sanity is enforced at build time: `tools/guest-builder/verify-root.sh` fails if containers/images/named-volumes != 0 or test residue remains. `sanitize-root.sh` provides deterministic cleaning.

## 15. Signing and distribution status

What is true now:

- Local/ad-hoc application signing has been exercised (`codesign --sign -` via Tauri, `harpoon/build/harpoon` ad-hoc with `harpoon/entitlements.plist` `com.apple.security.virtualization`). `codesign -dv --verbose=4` shows `adhoc,linker-signed` and `codesign --verify --deep --strict` passes for `Harpoon.app`.
- The Harpoon runtime **requires** the virtualization entitlement (`com.apple.security.virtualization`); the Tauri desktop executable does **not** receive it.
- Packaged application execution has been proven (`Harpoon.app` launches, `harpoon doctor` from bundle shows `binary .../Resources/harpoon/bin/harpoon` and `kernel .../Image-virt` at bundle path).
- Self-contained packaged runtime has been proven (the app carries `harpoon`, `Image-virt`, `harpoon-initramfs.cpio.gz`, `harpoon-root.img`; writable state is outside the bundle).

What is **not** claimed:

- **Developer ID signing** and **Apple notarization** are **not** complete for public distribution. Local ad-hoc / `Apple Development` signing (any valid Apple Development identity if available) is sufficient for local acceptance, but public macOS distribution via Gatekeeper requires a `Developer ID Application` certificate and `xcrun notarytool` / `stapler` notarization, which is separate from building Harpoon from source.

Building Harpoon from source does not require Developer ID or notarization; those are distribution-time steps.

## 15. Minimum macOS and standalone bundle contract (v0.1.1 Stage 2)

**Minimum:** `HARPOON_MIN_MACOS=15.1` — single source `harpoon/MIN_MACOS`.

- Tauri `bundle.macOS.minimumSystemVersion` = `15.1` (`tauri.conf.json`) → `LSMinimumSystemVersion` in `Info.plist`.
- Swift `harpoon/build/harpoon` built with `MACOSX_DEPLOYMENT_TARGET=15.1` and `xcrun swiftc -target arm64-apple-macosx15.1`, `LC_BUILD_VERSION` `minos 15.1` `sdk 26.5`. RPATHs: `/usr/lib/swift`, `@executable_path/../../../Frameworks`, `@loader_path/../../../Frameworks`.
- Virtualization.framework APIs used (`VZVirtualMachine`, `VZLinuxBootLoader`, `VZNAT`, `VZVirtio*`, balloon) are available since 11–13; 15.1 safely covers them. Tauri 2 requires 10.15+, so 15.1 is the binding floor. The acceptance machine on 15.1 is explicitly supported.

**Self-containment:**

```
Harpoon.app/Contents/
  MacOS/harpoon-desktop          # 15.1, arm64, no Swift deps
  Resources/harpoon/bin/harpoon  # 15.1, arm64, Swift, @rpath to Frameworks, entitlement virtualization
  Resources/harpoon/lib/harpoon/ # Image-virt, harpoon-initramfs.cpio.gz, harpoon-root.img (via assets/guest)
  Frameworks/                    # Swift runtime if embedding needed (via swift-stdlib-tool); currently OS-provided for 15.1
```

The nested `harpoon` is production executable code, not opaque data. Its location `Resources/harpoon/bin/harpoon` is explicit and resolved first by `RuntimeConfig.installedLibDir` (bundle `Resources/harpoon/lib/harpoon` before `~/Library` or `CWD`). No hard-coded Xcode paths, no `DYLD_LIBRARY_PATH`.

**Swift embedding:** For `15.1`, Swift stdlib is expected in OS `/usr/lib/swift` (Swift 6.0 runtime on 15.1). `harpoon/build.sh` adds Frameworks RPATH; if a future Swift version requires libs not in OS, `xcrun swift-stdlib-tool --copy --platform macosx --scan-executable harpoon/build/harpoon --destination Harpoon.app/Contents/Frameworks` will embed them (requires Xcode, not just CLT; CLT's `swift-5.0` lacks Swift 6 dylibs). No unnecessary libs are bundled.

**Verification:**

```sh
bash tools/verify-bundle.sh [Harpoon.app]
# checks: nested runtime, guest assets, arm64, minos 15.1 (not 26), dylibs system/@rpath, no DarwinFoundation1, RPATH Frameworks, entitlements, no spike/repo paths, Info.plist LSMinimumSystemVersion
```

Outside-repo proof:

```sh
cp -R Harpoon.app /tmp/harpoon-standalone-test/Harpoon.app
env -u HARPOON_BIN /tmp/harpoon-standalone-test/Harpoon.app/Contents/Resources/harpoon/bin/harpoon doctor
# must show: binary /tmp/.../Harpoon.app/.../harpoon, kernel /tmp/.../Harpoon.app/.../Image-virt, etc., no dyld failure
```

## 16. Clean build recipe

Concise copy/paste from repository root (verify paths exist before running):

```sh
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
nvm use 20

bash tools/guest-builder/build.sh
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
