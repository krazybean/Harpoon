# Desktop confirmation flow: isolation mechanism, security review and the isolated Harpoon Hello validation procedure

Status: **prepared, not run.** Everything here was verified offline (unit tests, a build, and a fail-closed startup check). No Docker-changing UI
test has been run, the serving engine (PID 8964) was not touched, and `Gate::shipping()` and Gate U are as in the gate-opening PR (#158).
The P1 build-overhead measurement is **NOT RUN and deferred for this release candidate by owner decision (2026-11-04)**. It is not a pass and
no 1.2x claim is made (see `DEPLOY_MILESTONE_B_ACCEPTANCE_REVIEW.md` section 14).

## 1. The problem this solves

Before this change the desktop was hard-wired to the serving engine: `deploy_op` read `~/Library/Application Support/Harpoon/runtime.json`, used the
constant `/tmp/harpoon-mgmt.sock`, kept its deployment store in the real app data directory, used the real Keychain, and every Docker call was
`docker --context harpoon`, i.e. the serving engine's socket. An isolated-engine UI test was therefore impossible without touching the serving engine.

## 2. The mechanism (`src-tauri/src/namespace.rs`)

* **Debug builds only.** A release build's `namespace::active()` is the constant `Ok(None)`: it does not read `HARPOON_TEST_TMPDIR` at all, so no environment
  variable can redirect a shipped desktop. A test pins that the release function contains no `env::` access, and `resolve(value, debug_build=false, …)` returns
  `None` for any value, valid or garbage.
* **Fail closed.** In a debug build, an unusable `HARPOON_TEST_TMPDIR` is an error, not a fallback to production. The app aborts in its setup hook, and every
  command that needs the namespace reports why. Proven by running a debug build with four unsafe values: each aborted with the reason and left
  `runtime.json` (sha256 prefix `39203f44…`) and the serving engine untouched.
* **Rules (mirroring the engine's `HarpoonRoots.isolationError`):** absolute path; must exist and be a directory; not `/tmp` or a symlink to it; not, inside, or containing
  `~/Library/Application Support/Harpoon` or `…/com.harpoon.desktop` (symlinks resolved); not writable by group or others; owned by the user running the app;
  short enough for a unix socket path (100-byte budget, same as the engine).
* **Everything is derived from the root:**

| Thing | Production | In a namespace |
|---|---|---|
| Engine state (`runtime.json`, `runtime.pid`) | `~/Library/Application Support/Harpoon` | the root (where the engine writes them under `HARPOON_TEST_TMPDIR`) |
| Management socket | `/tmp/harpoon-mgmt.sock` | `<root>/harpoon-mgmt.sock` |
| Docker socket | named by `runtime.json` | must equal `<root>/harpoon-docker.sock`, else refused (`confine_to_namespace`) |
| Forwarder pid / command | `runtime.json` + `runtime.pid` | the same files, in the root; the process must be `harpoon` |
| Resource directory (Buildx) | bundle `Resources`, or `bundle-resources` in a debug build | unchanged; chosen by the build, never by the variable |
| Desktop deployment store, attached runtimes | `app_data_dir()` | `<root>/desktop-data` |
| Secrets | the macOS Keychain | an in-memory store that dies with the process |
| Docker CLI | `docker --context harpoon …` | `docker -H unix://<root>/harpoon-docker.sock …` with an empty `DOCKER_CONFIG`; `DOCKER_HOST` / `DOCKER_CONTEXT` removed; a call that does not name the Harpoon context is refused |
| `harpoon` CLI (status, start, stop, disk) | inherits the environment | the same: the engine honors the variable (#155) |

* **Refused in a namespace** (they would reach the serving engine or the user's Docker setup): setting up the `harpoon` Docker context, running a command in containers, a
  container shell, Docker context discovery, and the installer for the command-line tool (skipped at launch). Compose launch fails closed because its context check does not name the
  Harpoon context.
* The deployment window shows a **"Test namespace"** banner with the root, so the operator can tell at a glance.

### Tests (`cargo test --locked`: 123 passed; frontend: 221 passed)

* `namespace::tests`: release builds ignore the environment; unset/empty is none; every path derives from the root and none contains `Library/Application Support`; relative,
  missing, `/tmp`, symlink-to-`/tmp`, production dirs (inside, symlinked, containing), non-directory, group/world-writable, wrong owner and too-long roots are errors; Docker calls are
  translated or refused and the command environment is confined.
* `deploy_api::tests::namespaced`: production locations unchanged without a namespace; a namespace selects its own `runtime.json`, management socket, Docker socket, forwarder pid and
  command and never reads the production files (a production-looking `runtime.json` with another pid is present and ignored); the resource directory is not derived from the variable; a state
  file pointing outside the namespace is refused; a non-engine process is refused; the Keychain is constructed in exactly one place and the namespace branch uses the in-memory store; the
  main-process guards are present.
* Frontend: the banner is read only from the backend, and no front-end file mentions the variable.

### Limits of the isolation (read-only reach that remains)

* `host_tcp_listeners` runs `lsof`, so the Containers/Deployments views can show the serving engine's listeners. It is read-only.
* The desktop and the serving engine's `harpoon` CLI still resolve the **binary** the same way (`HARPOON_BIN`, the checkout's `harpoon/build/harpoon`, …). With the variable set that binary acts on the
  namespace, but it is the same file the serving engine runs; the procedure below launches the desktop only after recording that file's hash.
* The macOS WebKit profile (`~/Library/WebKit/harpoon-desktop`) and window state are shared with a normal debug desktop. They hold UI state (e.g. the walkthrough marker), not deployment records.
* Two desktops cannot run at once: both claim `127.0.0.1:7480` (the application router) and port 1420 (Vite). The development desktop must be quit first.

## 3. Security review: Tauri command permissions and the `deploy_op` confirmation boundary

**Facts (read from the repository).**
* One window, `main`; capability `default` = `core:default` + `core:event:allow-listen`. No remote capability, no shell/fs/http/opener plugin.
* No app manifest restricts commands, so every registered `#[tauri::command]` is callable from the `main` window (Tauri v2 default). `app.security.csp` is `null` and `withGlobalTauri` is `true`:
  any script running in the web view can call every command.
* The web view loads local assets (release) or `http://localhost:1420` (debug). No remote content is loaded. The front end has no `dangerouslySetInnerHTML`, `innerHTML`, `insertAdjacentHTML`,
  `document.write`, `<iframe>`, `eval` or `new Function` (grep of `src`); logs, commit messages, repository and container names are rendered as React text. The one `window.open` is a development
  fallback; `open_app_url` accepts only Harpoon's own addresses.
* The same boundary already exposes stronger powers than `deploy_op`: `open_shell_session` (an interactive shell in a container), `run_universal_command` (commands in containers),
  `remove_image`/`remove_volume`/`remove_network`, `stop_harpoon`/`restart_harpoon`.

**Can untrusted content invoke `deploy_op`?** Not through any path found: untrusted text (logs, commit messages, names) reaches the web view only as escaped text, there is no HTML sink, and no remote
page is loaded. A script could call it only after an injection into the web view (a future XSS, or a compromised front-end dependency). That is the application's trust boundary, and at that point the
attacker already holds the container shell and `run_universal_command`. The confirmation is therefore not the weakest link, and the change makes it no weaker: the engine phrase exists only in the Rust backend; the
confirmation is a separate typed argument accepted only for Docker-changing operations; a cutover without it, or on anything but a deployment, is refused; Cancel sends nothing.

**Findings.**
1. *No actual vulnerability in the new confirmation path.* No correction is proposed in this PR.
2. *Defense in depth (not a vulnerability by itself): `csp: null`.* A restrictive CSP (`default-src 'self'`; `connect-src ipc: http://ipc.localhost`) would limit the blast radius of a future injection. It
   changes how the web view loads in development and release and needs its own test; **proposed as a separate hardening change, not made here**.
3. *Pre-existing, outside this change: `HARPOON_BIN` is honored in every build* (`resolve_harpoon_binary`). A process that controls the desktop's launch environment can choose which `harpoon` binary it runs; it
   already controls the process. Restricting that to debug builds is a one-line hardening, **not done here** to keep this change focused.
4. *A single-use token issued when a dialog opens, or a native confirmation sheet,* would additionally defend against an in-process script. Not proportionate to the risk above; not implemented.

## 4. Bounded Harpoon Hello UI validation on an isolated engine (needs separate written authorization; not run)

**Scope:** one sample application, one isolated engine, the desktop built from `feat/enable-attended-deployment`. Nothing runs against the serving engine. The sample is a copy: `~/Projects/harpoon-hello` is not touched
(it is not a Git repository and must not become one as a side effect).

### 4.0 Preflight (stop on any failure)
1. Record the serving fingerprint (use the `serving-check.sh` from `~/harpoon-b-acceptance-evidence/L567-20261103/`): PID 8964 and start time, `harpoon/build/harpoon` sha256 `711581b6…`, `/tmp/harpoon-docker.sock` inode, the serving
   disk inode, the container list (10, all exited), images (8), networks, `docker context ls` (default / desktop-linux / harpoon), plus the sha256 of `~/Library/Application Support/Harpoon/runtime.json`,
   and of the desktop's real store `~/Library/Application Support/com.harpoon.desktop/deploy/deploy-journal/*`. Free disk at least 25 GiB (stop below 15 GiB).
2. The owner quits the development desktop (`tauri dev`, PID 30867 today) and its Vite server: otherwise ports 7480 and 1420 are taken.
3. Build the shipping CLI for read-only checks and teardown: `cd deploy-cli && cargo build --release --offline` (record sha256, source commit and `rustc -V`).
4. `export EV=~/harpoon-b-acceptance-evidence/UI-<date>` (0700, never overwrite a non-empty step directory). `mkdir -m 700 /tmp/hpui` (short path: socket limit). The root must not be `/tmp`, must be owned by you, mode 0700.
5. Make the sample: `cp -R ~/Projects/harpoon-hello $EV/hello && cd $EV/hello && git init -b main && git add -A && git commit -m "hello v1"`. Later, `hello v2` changes only the page text.

### 4.1 Start the isolated engine
```
cp -p harpoon/build/harpoon $EV/engine/harpoon                 # a copy named "harpoon": the runtime check requires that name
HARPOON_TEST_TMPDIR=/tmp/hpui $EV/engine/harpoon start --cpus 2 --memory 2048 --disk-size 8G \
   --kernel assets/guest/Image-virt --initramfs assets/guest/harpoon-initramfs.cpio.gz
HARPOON_TEST_TMPDIR=/tmp/hpui $EV/engine/harpoon status --json     # isolated:true, every path under /tmp/hpui, pid != 8964
DOCKER_CONFIG=$EV/dockercfg docker -H unix:///tmp/hpui/harpoon-docker.sock pull node:22-alpine   # Hello's Dockerfile base; Harpoon builds with --pull=false
```
(Run `start` before any `status --json` in a fresh root, or the CLI pre-creates a default 32 GiB disk and `--disk-size` conflicts.)

### 4.2 Start the desktop in the namespace
```
cd ui/harpoon-desktop && HARPOON_TEST_TMPDIR=/tmp/hpui npm run tauri dev
```
Expect the **"Test namespace /tmp/hpui"** banner on Deployments. Containers must be empty. If it is absent, or any serving container is visible: **stop**.

### 4.3 The checks (record the journal sha256, the isolated container/image lists, `lsof` for ports 38000–38999 and 7480, and the serving fingerprint after each)

| # | Action | Pass |
|---|---|---|
| T0 | Add the application: Git folder `$EV/hello`, container port 43871, health path `/health`, branch `main` | recorded; no deployment, no container |
| T1 **Cancel** | Open Deploy, read the plan, press Cancel (and again via Esc and a click on the backdrop) | journal sha256, isolated inventory and listeners identical before/after; no `--authorize` was ever sent |
| T2 **Confirm** | Deploy, confirm | build, preliminary and final checks succeed; `curl 127.0.0.1:<leased port>/health` = 200 with `{"status":"ok"}`; the only listener on the port is `127.0.0.1` (owned by the isolated engine); the LAN address is refused; `docker inspect` shows the three ownership labels |
| T3 **Stable URL** | Press Open next to Application URL | the browser opens `http://<id>.harpoon.localhost:7480` and shows "Hello from Harpoon"; `curl -H 'Host: <id>.harpoon.localhost:7480' 127.0.0.1:7480/health` = 200 |
| T4a **Acknowledgement gates the cutover** | Commit "hello v2"; open Deploy again | "Replace and switch" is disabled until the acknowledgement is ticked; "Prepare only" is enabled; closing and reopening the dialog shows it un-ticked |
| T4b **Prepare only** | Press Prepare only | the running release's container id, `StartedAt` and restart count are unchanged; the stable URL still serves v1 throughout (probe 1/s, zero failures); a candidate image is retained; no pause is recorded; no stop of the running container |
| T4c **Replace** | Tick the acknowledgement, Replace and switch | v2 serves on the stable URL; a measured interruption is recorded; the previous container is stopped, not removed |
| T5 **Unattended denied** | Press Check for new commits; then with the shipping CLI: `harpoon-deploy git-run --runtime-dir /tmp/hpui/desktop-data/deploy …` | polling queues events and deploys nothing; `git-run` exits 6 with "not enabled in this build … Gate U"; there is no UI control that runs the queue |
| T6 **Exact-ID teardown** | For each recorded deployment: `harpoon-deploy teardown --deployment <exact id>` as a dry run, compare every target id with the journal/`docker ps`, then `--apply` with the authorization flag; `cleanup-image` (dry run first) for the retained candidate image | only session-owned resources are removed, by exact id; no prune, no force; `inventory` shows the isolated engine empty of Harpoon-labelled resources |

### 4.4 Teardown of the test and final proof
1. Quit the desktop. `HARPOON_TEST_TMPDIR=/tmp/hpui $EV/engine/harpoon stop`. Copy logs and the isolated journal into `$EV`, then remove `/tmp/hpui` only after `realpath` equals `/private/tmp/hpui`.
2. Re-record the serving fingerprint and compare with 4.0: PID, start time, binary hash, socket and disk inodes, container/image/network lists, Docker contexts, `runtime.json` hash, and the real desktop store hash must be **identical**. `shasum -a 256 -c` the evidence manifest.
3. The owner restarts their own development desktop afterwards.

### 4.5 Stop conditions
Any serving-fingerprint difference; the namespace banner missing; the Containers view showing serving containers; a listener on 7480 that is not this desktop; a confirmation sent without a click; a cutover without the acknowledgement; a second restoration attempt;
any resource outside the sample's application id; free disk below 15 GiB; the isolated guest `dockerd` failing to start (report, no retry without authorization). Do not rerun failed steps.

### 4.6 Authorizations this procedure needs
Quit the development desktop; start and stop one isolated engine in `/tmp/hpui`; pull `node:22-alpine` into it; real Docker changes inside the isolated engine only (the sample's build, containers and leases); the CLI teardown/cleanup with the explicit
authorization flag, dry run first; opening the stable URL in the default browser. Estimated duration: 60–90 minutes.
