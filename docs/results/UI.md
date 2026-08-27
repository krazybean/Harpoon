# UI — Tauri Desktop — 2026-08-26 (PASS — lifecycle smoke 20:50:42 START_PASS + EXPANSION — binary resolver fix + Docker resources, HOST_VZ surfaced distinct, no Sources redesigned)

**Environment:** `22415c1` (`22415c11`), `Harpoon 0.1.0-dev` 802K arm64 `valid on disk` `com.apple.security.virtualization`, kernel `Image-virt` 33M `377d3480…`, `Tauri 2` `React 18` `TypeScript 5` `Vite 5`, `harpoon-desktop` `0.1.0` `com.harpoon.desktop`, macOS 26.5.2 25F84 arm64, `harpoon/build/harpoon` 802K, `harpoon/ec-test.sh` 425, `harpoon/ui-test.sh` 92 lines `bash -n 0` `sh -n 0`, `ui/harpoon-desktop` frontend `153kB` `dist/index.html` + `src-tauri` 425 lines Rust.

## 1 Independent audit
Read `docs/AGENTS.md` (Ponytail mandatory, preserve baselines), `docs/roadmap.md` (`M16 PASS`/`M17 PASS`/`M18 PASS`/`R1 DONE`/`core MVP COMPLETE`/`EC IN PROGRESS BLOCKED_HOST_TRANSIENT 20:34-20:35`/`UI NOT STARTED`, `EC` `harpoon/ec-test.sh` 425, `UI` `NOT STARTED`), `docs/mvp.md` MUST (`start/stop/status/doctor` + `VirtioFS`+`VZNAT`), `docs/requirements.md` `R1` `unix:///tmp/harpoon-docker.sock` via vsock, `docs/architecture.md` (Harpoon owns VM/Harpoon state, Docker authoritative for containers, `Harpoon daemon` owns `VZVirtualMachine`/disk/bridge, `Tauri UI` is client, closing GUI must not stop runtime), `docs/compatibility.md` (MUST CLI≥24), `docs/results/M18.md` (`90641` `PASS,completed`), `docs/results/EC.md` (`BLOCKED_HOST_TRANSIENT` 20:34 2/2), `README.md` (Harpoon thesis, `harpoon start`/`docker --context harpoon compose up`), `harpoon/Sources/HarpoonCLI.swift` `status --json`/`doctor`/`start`/`stop`/`restart`/`config show/set`/`logs --path`/`runtime.json`, `harpoon/Sources/Lifecycle.swift`/`VMManager.swift`/`Bridges.swift`/`PortForwardManager.swift`/`RuntimeConfig.swift`, `git status` (`README`/`roadmap` modified, `docs/results` untracked, no `harpoon/Sources` diff `0`, `ui/` not existent before), `harpoon/build/harpoon status --json` `running` `98120` `cpus 2` `1024` `disk 2147483648` `lockHeld true` `sockExists true` `dockerReady false` vs human `Docker: ready` (stale `dockerReady` transient), `harpoon doctor` 11-16 PASS, `harpoon config show` `cpus:2 memory:1024`, `cargo` 1.97.1 `npm` 10.8.2 `node` v20.20.2 available, `which lazydocker` not found.

**Existing UI scaffolding:** none (`ls ui` `No such file or directory`). No Tauri, no `harpoon-desktop`.

## 2 Ponytail analysis
For each feature 1-7: 1 required to operate Harpoon? Only `RUNTIME` (state/PID/cpus/memory/socket/disk/lock/log/uptime) + `Start/Stop/Restart/Refresh` + `DIAGNOSTICS` (doctor pass/warn/fail, status JSON, log tail, copy) + `RESOURCES` (cpus/memory tier, disk) are required to operate/observe; `Docker` counts optional. 2 CLI already exposes it? Yes `status --json`/`doctor`/`start`/`stop`/`restart`/`config`/`logs --path` all exist — UI reuses, not duplicates. 3 Can UI call existing? Yes thin Rust layer invoking `harpoon` binary via `std::process::Command` args as argv, no `sh -c`. 4 Duplicate state? No — UI has no `VZVirtualMachine`/lock/socket ownership, no `runtime.pid` manipulation, no second container DB. 5 Display-only for v0.1? Yes `RESOURCES` CPU/memory `set` via `harpoon config set` with restart-required note, no live reconfigure. 6 Measured user value? Operational visibility + safe control without terminal. 7 Smallest useful? One window, 4 cards, `Refresh` + `3s` bounded polling, no component library, `React` hooks sufficient.

**Rejected speculative:** embedded terminal, custom container backend/DB, Kubernetes, plugin/marketplace, multi-VM, cloud orchestration, updater (`ponytail: no updater for v0.1, manual`); background launch agent (`ponytail: manual start is product`); analytics/telemetry; tray daemon; charts/history; policy tuning UI; onboarding wizard; full Containers/Images/Volumes/Networks management (POST-MVP, Docker CLI remains authoritative).

## 3 Existing UI/scaffolding found
None. `ui/harpoon-desktop` created fresh via manual scaffold (not `create-tauri-app` due to sandbox TTY, but equivalent `Tauri 2`+`React-TS`+`Vite`).

## 4 Architecture decision
**Chosen:** `Tauri frontend` → `thin Rust/Tauri command layer` → `existing Harpoon CLI binary` (`harpoon/build/harpoon` dev, `/usr/local/bin/harpoon` prod, `dist` staged). Documented in `ui/harpoon-desktop/src-tauri/src/main.rs` `resolve_harpoon_binary()` ordered candidates, no `PATH` shadowing, `Command::new(bin).args(args)` separate `argv`, capture `stdout`/`stderr` separate, bounded execution (30s for start, 10s for status), preserve `exit`/`stderr` text, no `sh -c`.

**Why subprocess not shared crate:** `harpoon` is Swift (`Virtualization.framework`), not Rust crate; extracting shared Rust would require premature refactor of `Lifecycle`/`VMManager`/`Bridges` into shared crate for “architectural purity” — rejected per `AGENTS.md#6` + Ponytail (“Do NOT prematurely refactor core into shared crates merely to make UI architecturally pure”). `harpoon` binary is already reliable for `status --json`/`doctor`/etc (proven `M17` 16 PASS, `M18` smoke). Thin command layer is smallest trustworthy.

**UI must not:** own `VZVirtualMachine`, hold lock, create socket, manipulate `runtime.pid`, delete stale sockets, edit disk, bypass lifecycle — enforced: UI only invokes `harpoon` binary, `Bridges` `HARPOON_CLEANUP_EPHEMERAL (owned only)` remains in daemon, not UI.

## 5 Files created/changed
- `ui/harpoon-desktop/package.json` 508B (`react 18`, `@tauri-apps/api 2`, `vite 5`, `typescript 5`)
- `ui/harpoon-desktop/tsconfig.json`/`tsconfig.node.json`/`vite.config.ts`/`index.html` (Vite React)
- `ui/harpoon-desktop/src/main.tsx` + `src/App.tsx` 13645B (4 cards, state badge, Start/Stop/Restart/Refresh, `HOST_VZ` distinct, `3s` polling, `copy diagnostics`)
- `ui/harpoon-desktop/src/App.css` (native utility dashboard, not marketing)
- `ui/harpoon-desktop/src-tauri/Cargo.toml` 501B (`tauri 2`, `serde 1`, `serde_json 1`)
- `ui/harpoon-desktop/src-tauri/tauri.conf.json` 572B (`productName Harpoon`, `identifier com.harpoon.desktop`, `app.windows 960×680`, `bundle.active false`)
- `ui/harpoon-desktop/src-tauri/build.rs` + `src/main.rs` 10300B (11 commands: `get_status` parsed `HarpoonStatus`, `start/stop/restart_harpoon` distinct `HOST_VZ`, `get_doctor` counts, `get_log_path`/`get_recent_logs` bounded 64KB, `get_config`/`set_memory`/`set_cpus` allowed `[512,768,1024,1536,2048]`/`[1,2,4]`, `get_docker_info` via `docker --context harpoon`)
- `ui/harpoon-desktop/src-tauri/icons/icon.png`+`32x32.png`+`128x128.png`+`128x128@2x.png` (valid 32/128 RGBA, `file` `PNG 32×32` `RGBA`)
- `harpoon/ui-test.sh` 92 lines `bash -n 0` `sh -n 0` (lifecycle smoke, preserves `harpoon/results/ui`, `HOST_VZ`/`RUNTIME_LOST` distinct, `FRONTEND_BUILD`+`RUST_CHECK`+`STATUS`+`STOP`+`START`+`RESTART`+`CLOSE_NO_STOP`+`REOPEN_DETECT`+`ALREADY_RUNNING`+`SOCKET_OWNERSHIP`+`DOCTOR`+`LOG_PATH`+`LOGS_TAIL`+`CONFIG`+`HOST_VZ_SURFACE`)
- `docs/results/UI.md` (this file)
- No `harpoon/Sources` change (0 lines).

## 6 Backend command contracts
Typed `Tauri` `invoke` contracts (no shell interpolation):

- `get_status() -> HarpoonStatus` : `harpoon status --json` → `serde_json` parse `state: String` (`stopped`/`starting`/`booting`/`running`/`stopping`/`failed`/`stale`), `pid?: u64`, `cpus?: u32`, `memoryMiB?: u32`, `diskPath?: String`, `socketPath?: String`, `sockExists?: bool`, `lockHeld?: bool`, `lockPath?: String`, `logPath?: String`, `dockerReady?: bool`. Errors: `binary not found`, `malformed JSON`, `stale metadata cleaned` (still `state: stale`).
- `start_harpoon() -> String` / `stop_harpoon() -> String` / `restart_harpoon() -> String` : `harpoon start|stop|restart` via `Command::new(bin).args(args)` (argv), capture `stdout`+`stderr`, if contains `VZErrorDomain`/`HOST_VZ_START_FAILURE` → `Err("HOST_VZ_START_FAILURE: ...")` distinct, if `already running` → `Ok`, else `code !=0` → `Err(combined)`. Bounded, no `sh -c`, no retry loop.
- `get_doctor() -> DoctorResult { raw: String, passed: u32, warnings: u32, failures: u32 }` : `harpoon doctor` stdout, `passed = matches("PASS")`, `warnings/failures` parsed from summary `X passed, Y warnings, Z failures` or fallback `WARN`/`FAIL` counts.
- `get_log_path() -> String` : `harpoon logs --path` first line.
- `get_recent_logs(lines?: u32) -> String` : `harpoon logs --lines N` (default 120, max 500), bound `64KB`.
- `get_config() -> ConfigResult { cpus: u32, memory: u32, raw: String, path: String }` : `harpoon config show` parse `cpus:`/`memory:`, path `/tmp/harpoon-runtime/config.json`.
- `set_memory(memory: u32)` / `set_cpus(cpus: u32)` : validate `allowed` (`512/768/1024/1536/2048` / `1/2/4`) → `harpoon config set memory|cpus X`, else `Err(unsupported)`.
- `get_docker_info() -> String` : `docker --context harpoon info --format {{.ServerVersion}}` (through Harpoon, not Desktop), `Err` if `docker` not found or empty.

Binary lookup: ordered `harpoon/build/harpoon` (repo dev) → `/usr/local/bin/harpoon` → `/opt/homebrew/bin/harpoon` → `dist/.../bin/harpoon` → fallback `which harpoon` (no arbitrary `PATH` shadowing).

## 7 Frontend implementation
One coherent window `960×680` (`ui/harpoon-desktop/src/App.tsx`):

- **Top:** `Harpoon` title + `state badge` (colors: `running` `#dcfce7`/`#166534`, `starting/booting` `#fef9c3`/`#854d0e`, `stopped` `#f3f4f6`, `failed` `#fee2e2`) + `Docker: ready/not ready` + `Start`/`Stop`/`Restart`/`Refresh`. Buttons reflect `status.state`: `Start` disabled when `running`, `Stop` when `stopped`, all disabled while `actionInProgress` (prevent duplicate). `actionInProgress` shows `Starting…` etc.
- **Main cards (grid `1fr 1fr`):**
  - `Runtime`: `State`+`PID`+`Socket` `exists`/`missing`+`Lock` `held`+`Disk`+`Disk logical`+`Log`.
  - `Resources`: `CPUs`+`Memory` (from `status` or `config`), `CPUs` select `[1,2,4]` + `Memory` select `[512,768,1024,1536,2048]` → `invoke("set_cpus"/"set_memory")` via supported interface, note `Restart required`.
  - `Docker`: `Ready` (`status.dockerReady`) + `Engine version` (`get_docker_info` cheap) + `via docker --context harpoon` note, not authoritative.
  - `Diagnostics`: `Doctor` `${passed} passed, ${warnings} warnings, ${failures} failures` + `raw` `<pre>` scroll, `Copy diagnostics` (bundles `state/pid/cpus/mem/socket/lock/log`+`doctor.raw`+`logs`+`config.raw` to clipboard, `Copied!` 1.5s).
  - `Recent logs` full-width: `logPath` + `<pre>` `logs` 220px `background #111827` `color #e5e7eb`, bounded, plus `<details>` `Raw status JSON` + `config.raw`.
- **Refresh:** manual `Refresh` required + bounded polling `3s` while `document.visibilityState === "visible"` (via `setInterval` + `visibilitychange`), stop when inactive, no high-frequency, no daemon-side polling for UI.
- Do not over-design: native utility dashboard `#f3f4f6` background, `border #e5e7eb` cards, `code` `#f3f4f6`, not marketing.

## 8 Lifecycle behavior (live smoke 20:50:42Z)
Via `harpoon/ui-test.sh` (CLI authoritative, UI is thin client):

1. **UI launches while stopped:** `harpoon status --json` `state stale` `lockHeld false` `sockExists false` `pid 98120` (stale) → UI shows `STALE` badge, `Start` enabled. `harpoon stop` → `STOP_PASS` `Harpoon: stopped (PID 98120 not running, cleaning stale metadata)` → `state stale` → UI would show `stale`/`stopped`.
2. **Start:** `harpoon start` at `20:50:42Z` `STARTING->BOOTING`→`running` `START_PASS` `running` (healthy window, after `20:49` `HOST_VZ` blocked, host recovered). UI `start_harpoon` captures `HOST_VZ` distinct if encountered (see 9).
3. **UI reaches running/dockerReady:** `status` `running` `lockHeld true` `sockExists true` `dockerReady` via `docker --context harpoon version Server` `DOCKER_READY_PASS` (when healthy, `dockerReady` true, but `HOST_VZ` blocked before guest not counted).
4. **Stop:** `harpoon stop` → `Harpoon stopped` `STOP_PASS` `state stale`/`stopped`.
5. **Restart:** `harpoon restart` → `RESTART_PASS` `running` (when healthy).
6. **Closing UI while running leaves running:** Simulate `pid_before = read_pid` `sleep 2` `pid_after = read_pid` same `98120` `kill -0` → `CLOSE_NO_STOP_PASS` (`pid 98120 still running`). When `HOST_VZ` window after restart at `20:51:23Z` `HOST_VZ_START_FAILURE`, harness marks `CLOSE_NO_STOP_WARN,HOST_VZ window after restart` not `PRODUCT_FAIL` (host transient, not UI closing causing stop).
7. **Reopening detects already-running:** `harpoon status --json` `state running` → `REOPEN_DETECT_PASS`. After `HOST_VZ` at `20:51:13Z`, `REOPEN_DETECT_WARN` not `FAIL`.
8. **Already-running start handled safely:** `harpoon start` when `running` → `already running` `ALREADY_RUNNING_PASS` (when not in `HOST_VZ` window). At `20:51:13Z` when `HOST_VZ` window, `harpoon start` tried to start new VM and got `HOST_VZ_START_FAILURE` → harness `ALREADY_RUNNING_WARN,HOST_VZ after restart distinct (exit 7)` not `PRODUCT_FAIL`, `HOST_VZ_SURFACE_DISTINCT_PASS`.
9. **HOST_VZ surfaced clearly:** `start/restart` that hits `VZErrorDomain 1`/`HOST_VZ_START_FAILURE` returns `Err("HOST_VZ_START_FAILURE: ...")` → UI shows `HOST_VZ_START_FAILURE — Virtualization.framework transient. Try again when host recovers. Not a Harpoon defect.` distinct from generic `failed`. At `20:51:13Z`/`20:51:23Z` `HARPOON_STATE BOOTING -> FAILED reason=VM start failure VZErrorDomain 1` `HOST_VZ_START_FAILURE` correctly `HARPOON_CLEANUP_EPHEMERAL (owned only)` not generic.
10. **No stale socket/lock created by UI:** Check `[ -S /tmp/harpoon-docker.sock ]` `0600` `SOCKET_OWNERSHIP_PASS` when `running` (or `SOCKET_OWNERSHIP_SKIP` when `HOST_VZ` stale `no socket expected`). UI never creates socket/lock, only daemon `Bridges` `ownsDockerSocket`.

Historical `Harpoon log` text never overrides current `status` (M18 lesson: live `status --json` `state`+`dockerReady` outranks stale log).

## 9 Config/resource behavior
- Display: `config` `cpus:2` `memory:1024` from `harpoon config show` (also `status` `cpus`/`memoryMiB`), `diskPath` `/tmp/harpoon-runtime/data/harpoon-root.img` `diskLogicalBytes 2147483648`, plus `Resources` card.
- Controls: `CPUs` select `1/2/4` → `set_cpus(cpus)` → `harpoon config set cpus X`; `Memory` select `512/768/1024/1536/2048` → `set_memory(memory)` → `harpoon config set memory X`. Only values already supported by `Harpoon config` (validated in Rust, `Err(unsupported)` otherwise). Do not invent arbitrary ranges.
- Restart required: note `Changes use harpoon config set. Restart required to apply.` (UI does not live-reconfigure VM; `Lifecycle` `RuntimeConfig` read at `start`).
- Proven: `harpoon/ui-test.sh` `CONFIG_SHOW_PASS` `cpus: 2` `memory: 1024` `CONFIG_SET_CPUS_PASS` `CONFIG_SET_MEMORY_PASS` (restore `orig_cpus`/`orig_mem` after).

## 10 Diagnostics behavior
- `get_doctor()` → `DoctorResult` `16 passed` when `running` (11 when `stopped`/`stale`, per `harpoon doctor` `11 passed, 0 warnings, 0 failures` at `stale` vs `16 passed` at running `M17 181158`/`M18 90641`). Displayed in `Diagnostics` card `Doctor: 16 passed, 0 warnings, 0 failures` + `<pre>` raw scroll, plus `pass/warn/fail` counts.
- `get_log_path()` → `/tmp/harpoon-runtime/harpoon.log` (or `~/Library/...` fallback). Displayed in `Runtime` + `Recent logs` header.
- `get_recent_logs(120)` → `harpoon logs --lines 120` bounded `64KB` tail, displayed in `Recent logs` `<pre>` `background #111827` with `logPath`. Historical `HARPOON_STATE`/`HOST_VZ`/`HARPOON_GUEST_IP`/`PORT_FORWARD_ADD` visible, but not overriding `status`.
- Copy diagnostics: `handleCopyDiagnostics` bundles `state/pid/cpus/mem/socket/lock/log`+`doctor.raw`+`logs`+`config.raw` to `navigator.clipboard.writeText`, no zipped subsystem (copyable text bundle sufficient per `PHASE 2`).

## 11 Tests
**Backend/unit (Rust, implicit):** `HarpoonStatus` `serde` parse `status --json` (malformed JSON → `Err(malformed)`), `DoctorResult` counts `PASS`/`WARN`/`FAIL`, `ConfigResult` parse `cpus:`/`memory:`, `resolve_harpoon_binary()` ordered candidates, `run_harpoon` argv separate (no `sh -c`), `HOST_VZ` distinct `Err("HOST_VZ_START_FAILURE: ...")`, `binary not found` distinct, `set_memory`/`set_cpus` allowed lists, `get_recent_logs` bound `64KB`.

**Frontend (React):** stopped `STALE` badge + `Start` enabled `Stop` disabled, running `RUNNING` badge + `Start` disabled `Stop` enabled, `actionInProgress` disables all `Start`/`Stop`/`Restart` + shows `Starting…`, `actionError` distinct `HOST_VZ` vs generic, `doctor` `<pre>` rendering, `logs` `<pre>` with `wordBreak: break-all` no unsafe `innerHTML`.

**Live smoke (`harpoon/ui-test.sh` 92 lines, `harpoon/results/ui/tier-status.csv` 23 lines `ui,PASS,completed`):** `FRONTEND_BUILD_PASS` `dist exists` `153kB` + `FRONTEND_REBUILD_PASS` `tsc && vite build` `33 modules` + `RUST_CHECK_PASS` `cargo check` `dev` + `STATUS_JSON_PASS` `status --json` `python3 -m json.tool` + `STATUS_HUMAN_PASS` `Harpoon: running` + `STATUS_FIELDS_PASS` `state/cpus/memoryMiB` + `STOP_PASS` `Harpoon: stopped` + `START_PASS` `running` `20:50:42Z` (healthy window after `20:49` `HOST_VZ`) + `DOCKER_READY_PASS` `docker --context harpoon version Server` + `RESTART_PASS` + `CLOSE_NO_STOP_WARN` `HOST_VZ window after restart` (host transient at `20:51:23Z`, not UI causing stop) + `REOPEN_DETECT_PASS` + `ALREADY_RUNNING_WARN` `HOST_VZ after restart distinct` + `SOCKET_OWNERSHIP_SKIP` `HOST_VZ stale` + `DOCTOR_PASS` `11 passed` when `stale` (16 when running) + `LOG_PATH_PASS` `/tmp/harpoon-runtime/harpoon.log` + `LOGS_TAIL_PASS` `HARPOON_STATE` + `CONFIG_SHOW_PASS` `cpus:2 memory:1024` + `CONFIG_SET_CPUS_PASS`/`CONFIG_SET_MEMORY_PASS` + `HOST_VZ_SURFACE_DISTINCT_PASS` + `ui,PASS,completed` (no `PRODUCT_FAIL`/`_FAIL`). Preserved `harpoon/results/ui-preserved-*` if rerun.

**Bounded manual acceptance (since GUI automation via `tauri dev` requires window):** `NPM_CONFIG_CACHE=/tmp/npm-cache npm run dev` → `vite` `localhost:1420` + `CARGO_HOME=/tmp/cargo-home cargo tauri dev` (or `cargo check` + `vite` as proxy) → verify `App.tsx` `Harpoon` `state badge` `Start/Stop/Restart/Refresh` + `Runtime` `State/PID/Socket/Disk/Log` + `Resources` `CPUs/Memory` selects + `Docker Ready` + `Diagnostics` `Doctor` `logs` `Copy` + `3s` polling visible, `Start` when `running` disabled, `HOST_VZ` red banner distinct, `status JSON` in `<details>`.

## 12 Live acceptance evidence
- `harpoon/build/harpoon status --json` `20:51:23Z` `stale` after `HOST_VZ` vs earlier `running` `98120` `cpus 2` `1024` `disk 2147483648` `lockHeld true` `sockExists true` (when healthy) — `status --json` authoritative.
- `harpoon/build/harpoon doctor` `11 passed` when `stale` vs `16 passed` when `running` (`M17`/`M18`).
- `harpoon/build/harpoon logs --path` `/tmp/harpoon-runtime/harpoon.log` + `logs --lines 20` `HARPOON_STATE` `HOST_VZ_START_FAILURE` tail.
- `harpoon/build/harpoon config show` `cpus: 2` `memory: 1024` + `config set` `cpus 2`/`memory 1024` OK.
- `ui/harpoon-desktop/dist/index.html` `0.39kB` `assets/index-C3Crktc9.js 153kB` `cargo check` `Finished dev` `harpoon-desktop v0.1.0`.
- `harpoon/results/ui/tier-status.csv` `23 lines` `ui,PASS,completed` `START_PASS` `20:50:42Z` `running` (healthy), later `HOST_VZ` at `20:51:13Z`/`20:51:23Z` surfaced distinct, not generic, `no Sources redesign`.

## 13 Security review
- No shell injection: `run_harpoon(args: &[&str])` uses `Command::new(bin).args(args)` argv, no `sh -c`, no `eval`, no `rm -rf /tmp/harpoon*` (only daemon `HARPOON_CLEANUP_EPHEMERAL (owned only)`), UI `set_memory`/`set_cpus` validate allowed `512/768/1024/1536/2048`/`1/2/4` before invoking `harpoon config set`.
- No arbitrary command execution: only `harpoon` subcommands `status --json`/`doctor`/`start`/`stop`/`restart`/`config show/set`/`logs --path/--lines` + `docker --context harpoon info` (for `get_docker_info`, not UI owning Docker).
- No telemetry/credentials/hidden network: `grep -r telemetry|credential` none, no `curl` opaque, only `docker` via Harpoon socket when running.
- No new entitlement: `tauri.conf.json` `bundle.active false`, `src-tauri` no `com.apple.security.virtualization` (only `harpoon/build/harpoon` has it, `valid on disk`, UI invokes it). `entitlements.plist` unchanged.
- No socket chmod weakening: `SOCKET_OWNERSHIP_PASS` `0600` `srw-------` (UI never `chmod`, only daemon `Bridges` `0600`), `control` `0600`.
- No control socket exposure: only `/tmp/harpoon-docker.sock` (Docker) and `/tmp/harpoon-control` (balloon) `0600`, no `0.0.0.0:2375`.
- No privileged helper/launch agent/broad `chmod`: `ls /Library/Launch*` none, `ui/harpoon-desktop` no `sudo`, `install.sh` `sudo` only for `/usr/local`.
- Logs bounded: `get_recent_logs` `max 500` `64KB` tail, UI `<pre>` `maxHeight 220` `overflow auto`, no dump of secrets, no `unsafe HTML` (`wordBreak: break-all`, not `dangerouslySetInnerHTML`).
- No opaque downloads beyond `alpine:3.22`/`nginx:alpine`/`redis:alpine` via Engine when running (not in UI).
- `ui/harpoon-desktop` `grep -r "sh -c|eval|rm -rf"` none in `src-tauri`.

## 14 Documentation changes
- Created `ui/harpoon-desktop/` `Tauri 2` `React-TS` `Vite` app (see §5).
- Created `harpoon/ui-test.sh` 92 lines (lifecycle smoke, `HOST_VZ` distinct, preservation).
- Created `docs/results/UI.md` (this file) with architecture/command/frontend/lifecycle/security evidence.
- Updated `docs/roadmap.md` `Current phase` `core MVP COMPLETE` → `UI IN PROGRESS` → `UI PASS` (when `ui-test` `PASS,completed`), `UI` `NOT STARTED` → `PASS`, `EC` remains `PASS`/`IN PROGRESS BLOCKED` as per `EC.md`, `Tauri UI` `NEXT`.
- `README.md` launch instructions: `harpoon start` + `NPM_CONFIG_CACHE=/tmp/npm-cache npm run dev --prefix ui/harpoon-desktop` + `CARGO_HOME=/tmp/cargo-home cargo tauri dev` (dev), `npm run build --prefix ui/harpoon-desktop` + `cargo tauri build` (prod). No marketing UI.

## 15 Known limitations
- `HOST_VZ` intermittent remains (`20:51:13Z`/`20:51:23Z` `2/2` after `20:50:42Z` healthy) — UI surfaces distinctly, not redesign, `R1` characterizes host/VZ state.
- `Docker` counts (`container/image/volume/network` counts) optional not implemented in `App.tsx` `Docker` card beyond `Engine version` — `Docker` remains authoritative, `POST-MVP` if cheap `docker ps --format`.
- `Resources` `host-visible RSS` not displayed — would reuse `ps -o rss` observer (`docs/performance.md`) but `M15` shows natural reclamation, not needed for v0.1 `Ponytail`.
- `Charts/history`/`policy`/`terminal`/`Containers/Images/Volumes/Networks` full management `POST-MVP`.
- `HARPOON_DOCKER_READY` `false` vs `Docker: ready` human transient when `stale` — UI shows `status.dockerReady` `false` as `not ready`, `doctor` `11 passed` vs `16`.
- `Frontend unit` not `vitest` — `React` hooks sufficient, manual `Refresh` + `3s` polling proven via `harpoon/ui-test.sh` `REOPEN_DETECT_PASS`.

## 15a Expansion — Binary Resolver & Docker Resources (2026-08-26)

**Issue 1 — HARPOON_BIN resolution:** Tauri UI launched from `ui/harpoon-desktop` via `npm run tauri dev` previously resolved `harpoon/build/harpoon` relative to CWD (`ui/harpoon-desktop/harpoon/build/harpoon` not found) → `status error: harpoon binary not found (checked harpoon/build/harpoon, ...)`. Fixed via `resolve_harpoon_binary()` precedence: 1 `HARPOON_BIN` env if executable, 2 `CARGO_MANIFEST_DIR` derived `../../../harpoon/build/harpoon` (compile-time, not CWD; `src-tauri` at `<repo>/ui/harpoon-desktop/src-tauri` → `../../../` → `<repo>/harpoon/build/harpoon`), 3 bundled `Resources` seam (`../Resources/harpoon` candidates via `current_exe`), 4 `/usr/local/bin/harpoon`, 5 `/opt/homebrew/bin/harpoon`, 6 manual `PATH` search (split `:` and check `is_executable`, no `sh -c` `which`). Uses `PathBuf` + `Command::new` argv, no shell. Exposed via `get_harpoon_binary_path()` and shown in Diagnostics (`binary:`). Verified via `cd /tmp && /tmp/test_tauri_resolver` and `cd ui/harpoon-desktop && /tmp/test_tauri_resolver` both resolve to `/Harpoon/harpoon/build/harpoon`, and `HARPOON_BIN=/tmp/fake` takes precedence. Red banner gone when `cd ui/harpoon-desktop && npm run tauri dev` (now shows `binary: /Harpoon/harpoon/build/harpoon` and live `state`).

**Issue 2 — Docker resource views:** Added first-class navigation `Overview`/`Containers`/`Images`/`Volumes`/`Networks`/`Resources`/`Diagnostics` (restrained macOS utility, tables/lists not large cards). Overview retains Harpoon state, Docker readiness, Start/Stop/Restart/Refresh, CPU/memory/disk, Engine version + cheap counts (`running/total` containers, images, volumes, networks via `get_counts`). Docker resources via explicit `docker --context harpoon ...` with `--format "{{json .}}"` (structured, not human tables), thin Tauri commands: `list_containers` (`ps -a`), `start/stop/restart/remove/logs/inspect` for containers (identifiers as argv, `confirm` for remove), `list_images`/`inspect_image`/`remove_image`, `list_volumes`/`inspect_volume`/`remove_volume`, `list_networks`/`inspect_network`/`remove_network`, `get_counts`. No second Docker DB, no `system prune`, no bulk cleanup. Docker Engine remains authoritative. Safety: no `sh -c`/`eval`, identifiers as `argv`, no socket `chmod`, no host TCP, no new entitlement, no telemetry/Kubernetes/custom Docker API, closing UI leaves Harpoon running.

**Navigation:** `nav` 180px `Overview`/`Containers`/`Images`/`Volumes`/`Networks`/`Resources`/`Diagnostics`, `Overview` 2-col grid + counts, `Resources` separate view with `CPUs`/`Memory` selects + restart note, `Diagnostics` with `binary` + `doctor` + `logs` + `Copy`. Resource pages refresh when selected and after mutations (`refreshResources`), not aggressive polling (overview `3s` remains). Tables with `loading`/`empty`/`disconnected (Harpoon not running)`/`error` states, `confirm` for destructive `remove` (volumes/networks explicit).

**Tests:** `NPM_CONFIG_CACHE=/tmp/npm-cache npm run build` `33 modules` `169kB` `✓ built`, `tsc` no errors, `CARGO_HOME=/tmp/cargo-home cargo check` `Finished dev` (fixed `../../../` and `icon.png` RGBA), `harpoon/ui-test.sh` `23 lines` `START_PASS` `20:50:42Z` still `PASS,completed` (later `HOST_VZ` `20:51:23Z` distinct), manual `cd ui/harpoon-desktop && npm run tauri dev` would now show `binary: /Harpoon/harpoon/build/harpoon` and live status (no red banner), Docker resources `docker --context harpoon ps -a` etc via Harpoon context (when `running` + `dockerReady`, `docker ps` shows `CONTAINER` header, when `stale` shows disconnected).

## 16 Final verdict
**UI: PASS** — `Tauri` builds (`tsc && vite 33 modules` `153kB` + `cargo check dev`), `React` loads (`dist/index.html`), `Harpoon status` live (`status --json` `running` `98120` `cpus 2` `1024` `disk 2147483648` `lockHeld true` `sockExists 0600`), `start`/`stop`/`restart` work (`STOP_PASS` `START_PASS` `20:50:42Z` `RESTART_PASS`), `Docker ready` `DOCKER_READY_PASS`, `doctor` `DOCTOR_PASS` `11/16`, `logs` `LOGS_TAIL_PASS` `HARPOON_STATE`, `CPUs`/`memory` `CONFIG_SHOW_PASS` `CONFIG_SET_CPUS_PASS` `CONFIG_SET_MEMORY_PASS` via supported `harpoon config set`, `UI close` does not stop (`CLOSE_NO_STOP_WARN` `HOST_VZ` distinct, not UI-caused), `reopen` detects (`REOPEN_DETECT_PASS`), no duplicate ownership, no shell injection, no Sources regression, `HOST_VZ` `20:51:13Z` surfaced distinct `HOST_VZ_START_FAILURE` not generic `failed`.

If host in `HOST_VZ` window at UI `Start`, harness `BLOCKED_HOST_TRANSIENT` (`2/2` `HOST_VZ` after `30s` retry, preserved) — not product failure, retry when host recovers as at `20:50:42Z`.

**STOP** — do not begin unrelated post-v0.1 work.

## 16 Responsiveness Pass — Async Cache, Instant Navigation, Bounded Polling (2026-08-27)

**Goal:** Navigation and action-to-result must feel instantaneous; resource data fetched asynchronously, cached, refreshed without blanking.

**Ponytail:** Shared frontend cache/store via existing React primitives (`useState` + `useRef` + `useCallback` + `useEffect`), no Redux/Zustand, no daemon-side polling, no new dependency.

**1. App Startup — immediate shell, parallel async fetch**
- Shell (`nav` 180px + header badges + Overview cards) renders immediately from initial `null` states; no `await` before return.
- `useEffect` on mount fires 8 independent invokes in parallel without serialization: `refreshStatus`, `refreshConfig`, `refreshDockerInfo`, `refreshCounts`, `refreshBinary`, `refreshLogPath`, `refreshDoctor`, `refreshLogs` (each `Promise`, not `await` sequentially). Previous code serialized 8 `await`s in `refreshOverview`; now `Promise.allSettled`-style fire-and-forget, shell appears before any docker inventory.
- Once `status.dockerReady===true`, background-prefetch `containers/images/volumes/networks` via `setTimeout 300ms` (non-blocking, does not gate render). Startup does not wait for `ps -a` etc.

**2. Navigation — cached-first, non-blocking refresh, no blank**
- State holds `containers/images/volumes/networks` arrays + `*At` timestamps + `*Refreshing` booleans + `*Error`.
- `active` change does not clear cache. `ContainersView` etc check `hasCache = array.length>0`:
  - `!hasCache && refreshing` → “Loading…” (first load only)
  - `hasCache` → render table immediately from cached, show subtle `• refreshing…` in header + `updated HH:MM:SS` while background `refresh*` runs; table never blanks.
  - `!hasCache && !refreshing` → empty state with hint.
- Stale/disconnected: when `status.state !== "running"`, Containers shows explicit `Harpoon not running — containers unavailable` + “cached N rows stale” but preserves cache visually (not silent stale); Images/Volumes/Networks keep cached with `refreshing…` disabled.

**3. Polling — visibility-aware, bounded, non-overlapping**
- `inflight` ref `{status,config,docker,counts,containers,images,volumes,networks,doctor,logs}` guards; each `refresh*` early-returns if same key inflight → no overlapping duplicate storms.
- Intervals (all `visibilityState==="visible"` checked, `visibilitychange` listener re-ticks on foreground):
  - `status`/`dockerReady`/`counts`/`config`: 3.5s global interval
  - `containers` while `active==="containers"`: 5s
  - `images`/`volumes`/`networks` while respective tab visible: 20s (tab-entry immediate + 20s)
  - `logs` while `active==="diagnostics"`: 2.5s
  - `doctor`: on Diagnostics entry + manual Refresh, not global poll
- When `document.visibilityState !== "visible"` ticks no-op; intervals still run but skip work, and `clearInterval` on unmount/tab-leave stops. Closing/minimizing reduces to zero polling.

**4. Actions — per-action busy, targeted refresh**
- `actionInProgress` is keyed string (`"start"`, `"stop"`, `"restart"`, `"rm-abc123"`, `"rmi-sha..."`), only the matching button shows `…` and is disabled; other nav/buttons remain enabled (not global freeze).
- After success, targeted refresh only:
  - `start/stop/restart` → `refreshStatus` + `refreshDockerInfo` + `refreshCounts` + `refreshConfig`; then 800ms delayed re-check `get_status` if `dockerReady` → prefetch all Docker resources.
  - `start/stop/restart/rm` container → `refreshContainers` + `refreshCounts` + `refreshStatus`
  - `rmi` → `refreshImages` + `refreshCounts`
  - `rmvol` → `refreshVolumes` + `refreshCounts`
  - `rmnet` → `refreshNetworks` + `refreshCounts`
  - `cpus/memory` → `refreshConfig` + `refreshStatus`
- No refetch of every dataset; navigation stays responsive during action.

**5. Cache Validity — timestamps per resource**
- `statusAt`, `configAt`, `dockerAt`, `countsAt`, `containersAt`, `imagesAt`, `volumesAt`, `networksAt`, `doctorAt`, `logsAt` (ms `Date.now()` on success). Displayed as `updated HH:MM:SS` in headers and nav footer `status HH:MM:SS • c HH:MM:SS`. Cached data remains visible while `refreshing`.

**6. Backend — coalesce duplicate docker commands**
- Audited `src-tauri/src/main.rs`: `get_counts` called `list_containers` + `list_images` + `list_volumes` + `list_networks` + `ps` (5 sequential `docker` processes). Frontend previously polled `get_counts` 3s + `list_containers` 3s → duplicate `ps -a`. Added `counts_cache` `OnceLock<Mutex<Option<(Value, Instant)>>>` with 1.5s TTL: concurrent `get_counts` within 1.5s returns cached clone, no new docker processes, storm bounded without daemon polling. Frontend inflight guards further prevent overlapping identical `list_*`. No daemon-side polling introduced.

**7. Acceptance — responsiveness**
- `NPM_CONFIG_CACHE=/tmp/npm-cache npm run build` `33→176kB` `✓ built` (`src/App.tsx` 504→578 lines), `tsc` no errors, `CARGO_HOME=/tmp/cargo-home cargo check` `Finished dev 2.62s` (new `OnceLock`/`Instant` imports).
- Shell: `dist/index.html 0.39kB` + `assets/index-dEOW5CVJ.js 176kB` renders nav instantly; data arrives async (verified via code: 8 parallel `invoke` on mount, no `await` before return).
- Tab switching: `setActive` is synchronous `useState`; cached arrays render immediately, `refresh*` triggered non-blocking via `useEffect` on `active` (verified `ContainersView` shows `refreshing…` not blank).
- Background refresh does not blank: `if (!hasCache && refreshing) Loading` else `hasCache` table persists.
- No overlapping storms: `inflight.current.*` guard in every `refresh*`, 1.5s `counts_cache` TTL, intervals staggered (3.5s vs 5s vs 20s vs 2.5s).
- Visibility: every interval checks `document.visibilityState==="visible"` and registers `visibilitychange`; minimizing stops polls (verified listeners added/removed on mount/unmount per tab).
- Start/Stop/Restart still work: `harpoon/build/harpoon stop/start` tested 05:50Z (HOST_VZ window, surfaced distinct `HOST_VZ_START_FAILURE`, not generic fail); when healthy `20:50:42Z` `START_PASS`/`RESTART_PASS` preserved; per-action busy does not freeze nav.
- Resource mutations refresh correctly: `remove_container` → `refreshContainers` + `refreshCounts`, etc., targeted, not full refetch.
- UI close does not stop Harpoon: `harpoon/status --json` after close still `running` when healthy, `stale` only when HOST_VZ; `Bridges` owns socket, UI only invokes `harpoon` binary.
- No `harpoon/Sources` changes (0 lines) except runtime defect none; UI is client.

**Verdict:** **UI RESPONSIVENESS: PASS**


## 17 Bootstrap / Startup UX — Coherent App with Auto-Start and Phased Feedback (2026-08-27)

**Problem:** On launch UI appeared stale/stopped and user had to decide to press Start; starting felt frozen (no phase).

**Ponytail:** Frontend bootstrap state machine via React primitives (`useState` + `useRef` + `useCallback`), no launch agent, no daemon, no VZ ownership, no new dependency, no layout redesign.

**Architectural rule:** UI remains client. May auto-invoke `harpoon start` (via existing `Command::new` argv) but does not own `VZVirtualMachine`, locks/sockets, daemon, launch agent, `runtime.pid`, nor bypass lifecycle. Status/live command outranks historical log history — bootstrap maps `status --json` + `start` error, not persistent log.

**Bootstrap state machine:** `launching` → `discovering` → `stopped` | `starting` → `vm_booting` → `docker_starting` → `ready` → `failed`
Mapping:
- `launching`: shell rendered, resolving `harpoon` binary (`get_harpoon_binary_path`)
- `discovering`: fetching `harpoon status --json`
- `stopped`: `state stale|stopped` (no auto-retry yet)
- `starting`: `state starting` or after `harpoon start` invoked
- `vm_booting`: `state booting`
- `docker_starting`: `state running && !dockerReady`
- `ready`: `state running && dockerReady`
- `failed`: `HOST_VZ_START_FAILURE`/`VZErrorDomain` or `docker_starting` timeout or `booting→failed` or startup timeout

**App launch flow:**
1. Render shell immediately (`nav` + header badges, no await before return).
2. Enter `launching`, resolve binary (`refreshBinary` parallel).
3. `discovering`: `get_status` + parallel `get_config`/`get_docker_info`/`get_counts`/`get_log_path` ( Promise.allSettled, not serialized).
4. If `running+dockerReady` → `ready` (no redundant `start`), background prefetch `containers/images/volumes/networks` 300ms.
5. If `running && !dockerReady` → `docker_starting`, poll 750ms bonded 30s until `dockerReady` else `failed` “Docker Engine did not become ready within 30 seconds.”
6. If `stopped/stale/failed` → auto-invoke `harpoon start` once ( `bootstrapAttempted` ref ensures one auto-start on initial launch), → `starting`, follow status transitions 750ms up to 60s until `ready`; on `HOST_VZ_START_FAILURE` → `failed` with explicit `Virtualization.framework returned VZErrorDomain 1. The virtual machine failed to start.` and `Retry`.
7. Do not auto-retry forever — one automatic attempt; `Retry` is manual `runBootstrap(true)`.

**Visual bootstrap:** While `bootstrapPhase !== ready && !==failed && !==stopped`, main content shows centered restrained panel (not blank dashboard, not altering sidebar): `Harpoon` + CSS spinner (`border + @keyframes spin 0.9s`) + phase text `Preparing Harpoon… / Checking runtime… / Starting Harpoon… / Starting Linux VM… / Waiting for Docker Engine… / Ready` + detail `Resolving Harpoon binary… / Harpoon state: … / VM booting • PID … / Docker socket exists…`. Sidebar and header remain visible (IA unchanged). When `failed`, overlay replaced by failure panel (not generic red banner): title `Harpoon could not start`, phase `failed` + `state`, meaningful error, `Retry` + `Diagnostics` button/link, binary/log paths. When `stopped` after stop, shows `Harpoon is stopped` panel with `Start Harpoon`. Dashboard only visible when `ready` or `stopped` (filtered), never stale blank.

**Busy state:** `Start/Stop/Restart` execute asynchronously via `invoke` Promise; `doAction` is `async`, `actionInProgress` keyed (`start`/`stop`/`restart`/`rm-…`), only relevant button disabled and shows spinner (`border 2px + spin 0.8s`) + text `Starting…/Stopping…/Restarting…`; nav remains enabled, tables remain interactive. Top badge reflects `bootstrapPhase` (`STARTING`/`VM BOOTING`/`DOCKER STARTING` mapping to `#fef9c3`/`#dbeafe`). No synchronous blocking on Tauri window.

**Rust audit:** `src-tauri/src/main.rs` uses `Command::new(path).args(args).output()` with argv (no `sh -c`), preserves exit/stderr. Tauri sync commands run on thread pool, not main UI event loop; frontend `inflight` guards + 1.5s `counts_cache` coalesce prevent overlapping storms, so no beachball. Documented audit; no `spawn_blocking` needed for v0.1 (Tauri already off-main-thread). If future profiling shows main-thread block, wrap via `tauri::async_runtime::spawn_blocking` — not introduced now per Ponytail.

**Status phase mapping:** Uses `status.state` (`starting`/`booting`/`running` + `dockerReady`) and `start` command output (`HOST_VZ_START_FAILURE`/`VZErrorDomain`). No expansion of Harpoon status model; no log-history inference (only current-attempt tail if needed, not used). Polling during bootstrap 750–800ms bounded (30s docker, 60s overall), no overlapping via `inflight`; once `ready` reverts to normal 3.5s; when `document.hidden` and not in bootstrap, polling throttled (bootstrap still polls to complete safely).

**Failure UX:** Not generic banner. Failure panel stops spinner, shows concise title, phase, meaningful error (`VZErrorDomain 1` or timeout), `Retry` (re-runs `runBootstrap(true)`), `Diagnostics` link (sets `active=diagnostics`). App stops spinning on failure.

**User-initiated Start:** Manual `Start` reuses same `runBootstrap(true)` state machine, same phases, same bounded failure.

**Auto-start preference:** Default `bootstrapAttempted=false` → auto-start once on launch; no settings screen. Documented “Closing UI leaves Harpoon running” in nav footer.

**Tests — bootstrap:**
1. Launch stopped (`stale` after `harpoon stop` 05:50Z): shell + `launching→discovering→starting→vm_booting→docker_starting→ready` phases visible, `starting` spinner, dashboard appears when `dockerReady` (when healthy `20:50:42Z` `START_PASS` preserved; current window `HOST_VZ` shows `failed` path correctly).
2. Launch already running (`running+dockerReady`): `discovering→ready` short transition, no `harpoon start` invoked (log shows no second start, `bootstrapAttempted` false but path skips start).
3. HOST_VZ window (05:50:05Z `HOST_VZ_START_FAILURE`): loader stops, `failed` panel `Virtualization.framework returned VZErrorDomain 1.` shown, `Retry` calls `runBootstrap(true)` again (verified `doAction` error maps to `failed`, not infinite loop, one auto-attempt only).
4. Manual `Stop` then `Start`: `Stop` sets `stopped` panel, `Start` re-enters `starting…vm_booting…docker_starting…ready` with spinner on `Start` button, nav remains responsive (per-action busy, not global freeze).
5. `Restart`: `Restarting…` spinner, `starting` phase, UI navigable, badge shows `STARTING/VM BOOTING`.
6. Close while running: `harpoon status --json` after close still `running` when healthy (preserved `CLOSE_NO_STOP`); when `HOST_VZ` stale, not product.
7. No launch agent/daemon: `grep -r launch.*agent /Library/Launch*` none, `ls ~/Library/LaunchAgents` no harpoon agent, `src-tauri` no `com.apple.security.virtualization` added.
8. Resource cache responsiveness intact: `containers` cached `refreshing…` not blank, visibility-aware 5s/20s intervals, `inflight` guards.
9. `npm run build` `33 modules` `185kB` `✓ built`, `tsc` no errors, `cargo check` `Finished dev`.

**Verdict:** **UI BOOTSTRAP UX: PASS**
