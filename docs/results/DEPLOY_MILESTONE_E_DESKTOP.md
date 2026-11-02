# Milestone E — real desktop deployment experience

Status: implemented and verified offline in one PR. Attended only. `Gate::shipping()` is unchanged and closed. Gate U is NOT PASSED.

## 1. Ponytail review

| Decision | Item |
|---|---|
| KEEP | The Deployments UI as built (screens, dialogs, parts, CSS, presentation vocabulary, `DeployState` contract); the engine crates; `run_with` as the one mutation path; `off_main`-style blocking work. |
| REUSE | `SqliteJournal::read_only` + `writer_lock_status` for state; `harpoon_deploy_cli::run_with` with `Gate::shipping()` and the real Keychain for mutations; existing `list_containers` / `logs_container` for Docker's current state and runtime logs; existing Containers navigation. |
| CUT | A daemon, HTTP/WebSocket, a second database, parsing human CLI output for state, a TS copy of engine rules, bundling the acceptance binary. |
| DEFER | Branch previews (section 6), per-application removal, resource charts, build-arg editing, SSH known-host entry in the dialog. |
| RISK | Presenting recorded state as current (answered by observation, section 3); a stale snapshot (marked); a process killed mid-deployment (reported as interrupted). |

## 2. Backend contract

`ui/harpoon-desktop/src-tauri/src/deploy_api.rs` (blocking work on `spawn_blocking`; operations serialized by a mutex, the journal's own lock covers other processes):

* `deploy_snapshot() -> EngineSnapshot` — read-only replay of `<app data>/deploy/deploy-journal`: `store` (`missing` / `ok` / `error` with the reason), `writerHeld`, `gate`, repositories (with last-seen commits), applications with deployments (steps, failure, cutover, recovery with reason, interruption, release, replaces, rollback target, source commit, pinned release config with secret *versions*, snapshot entries and diffs against the previous snapshot), serving release, pauses, held/selected events, and scoped definitions. Millisecond facts only; no secret value can occur in it (asserted).
* `deploy_step_logs(deployment)` — the recorded, redacted step logs.
* `deploy_op(op)` — `add-repo`, `repo-secret`, `set-config`, `set-secret`, `unset`, `deploy`, `run-queue`, `poll`, `rollback`, `restore`, `recover`, `resume`; answered with `{ok, exitCode, summary, detail, refusedByGate}` carrying the engine's own text. A secret value is only the standard input of the engine command.

Engine additions: `deplog` journal records (per deployment and step, 64 KiB tail, redacted, withheld when redaction could not be proven; fail-closed replay that every log names a recorded deployment) written after each deploy/replace/rollback; `watch --queue-only`.

## 3. Mock to real

`App.tsx` renders `DeployView` with `engine` (the `useDeployEngine` hook) unless a development build is opened with an explicit preview link; only then is `sim` used, with its controls and notice. `toDeployState` (pure, in `lib/deployEngine.ts`) adapts the snapshot to the existing `DeployState`:

* No snapshot → *loading*; failed first load → empty with the error; damaged store → nothing plus the reason. Last good data is kept and marked **stale** on a later failure.
* **Recorded vs current:** a serving release is *running* only when Docker was observed running its container; otherwise it is "last known" (down while connected, unknown while stopped). If Docker cannot be asked, the runtime is shown as not ready, not as running.
* A deployment that never finished with no writer holding the journal is **interrupted**, never running; a recovery recorded as in progress with no writer is shown as unknown, never "restoring"; an interruption with no end says its end is unknown.
* Removed in engine mode: fabricated health text, progress bars, resource figures, retention numbers, previews, mock Docker object names, "simulated" tags.

## 4. Working flows

Add repository (Git URL or local Git folder, branch, Dockerfile, port, health path, project, optional HTTPS token to the Keychain) → configuration and secrets (scoped, secret versions, snapshot diffs without values) → Deploy / Redeploy / Retry plan dialogs → deployment overview, history, recorded build logs, Docker's current application logs, "Open in Containers" cross-links → releases and rollback eligibility with the data-and-schema warning → attended polling (`Check for new commits`), paused triggers, held and selected events, explicit resume.

## 5. Shipping gate

`Gate::shipping()` is untouched. `deploy_op` is the only execution path and it cannot construct another gate; the acceptance binary is not referenced by the app (its Cargo feature is off). Deploy, Redeploy, Retry, Rollback and Restore are disabled with the gate's reason in the UI, and if invoked the engine refuses them (exit 6, `refusedByGate`) before reading anything; a refused deployment leaves no record (tested). Read-only inspection, repository/config/secret changes and polling work. No simulated success exists in engine mode.

## 6. Branch previews — boundary, not implemented

A safe preview needs (a) one registered application per branch with its own identity and loopback port, (b) a preview configuration scope, (c) a persisted expiry plus a sweeper that runs while Harpoon is open, and (d) cleanup restricted to Docker objects carrying verified Harpoon ownership labels. The engine has none of the four, and (a)+(c) are a new subsystem on the B14/D journal. Per the brief the core integration was finished first; the UI shows Preview and Development as unavailable and never lists a preview. Limits/deletion/expiration/interruption tests cannot exist for a capability that does not.

## 7. Tests

* Rust (`deploy_api::tests`, real SQLite journals): missing store; recorded deployments; unfinished with and without a writer (interruption); damaged store; gate refusals with the engine reason and no side effects; secret on stdin never in output or snapshot; locked Keychain reported, nothing recorded; repository registration then refused deploy; writer-held reads.
* Frontend (`deployEngine.test.mjs`, 15 tests; `deployIntegration.test.mjs` updated): loading/empty/error, no invented content, observed-vs-recorded status, stopped VM, interrupted and may-be-down deployments, failed build, restored cutover, lost recovery, pauses and held events, secrets as names/versions only, gate reason, log redaction notices, production uses the engine and simulation is preview-only.
* Engine: step-log unit and journal replay tests; the planted-secrets CLI test also asserts the recorded failed-build log is redacted.
* Not run: any live Docker mutation; the desktop was not launched against a live runtime in this PR.

## 8. Remaining acceptance limitations

Gate U NOT PASSED (attended only); a live deployment, rollback and restore from the desktop are unproven because the gate is closed; the Keychain "locked" state is learned from a failed engine call, not probed ahead; Docker state is polled (3 s while Deployments is open); previews, development deploys, application removal and settings editing are not available; Milestone D's residual risks stand (see the roadmap).

## 9. E.1 — runtime coordinates for Docker-touching operations

`deploy_op` used to pass no `--docker-socket`, `--mgmt-socket`, `--resources-dir`, `--forwarder-pid` or `--forwarder-command`, which the CLI requires (and never defaults) for `git-deploy`, `git-run`, `rollback` and `restore`. Now, for exactly those operations, `deploy_api` reads `~/Library/Application Support/Harpoon/runtime.json` (Docker socket, pid), checks it is still true (`runtime.pid` agrees; the pid is a running program named `harpoon`; the Docker and management sockets are sockets; the bundled `harpoon/bin/docker-buildx` is a regular file) and passes the values explicitly. The management socket is the runtime's fixed `/tmp/harpoon-mgmt.sock` (not recorded in `runtime.json`). Nothing comes from the environment, a Docker context or a default.

* **Authorization is not supplied.** `--authorize-live-docker-changes=…` remains the user's explicit act; the app never adds it.
* **Order of refusals:** while the gate is closed for an operation, the engine's gate refusal stays first and only (a missing runtime is not reported instead of it). Once a gate would let it through, a missing or stale runtime refuses it in the app before the engine is reached, with the reason.
* Polling, config, secrets, resume and recover get no Docker coordinates.
* The gate, the acceptance binary and the engine are unchanged; with `Gate::shipping()` no live operation can run, so this is verified offline (argument construction, discovery against temporary sockets and files, refusal ordering). Not proven: a live run with the gate open, and that `ps` reports `harpoon` for the installed app's runtime (it does for the current dev runtime).
