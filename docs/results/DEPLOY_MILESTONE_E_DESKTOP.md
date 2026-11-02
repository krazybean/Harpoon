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

## 10. E.2 — Git workspace paths with spaces

Closed-gate validation of E/E.1 found that polling from the desktop's real data directory (`~/Library/Application Support/com.harpoon.desktop/deploy`) failed: `Workspace::create` accepted only `[A-Za-z0-9/_.-]` in the whole workspace path. The restriction protected one real shell-parsed string, `GIT_SSH_COMMAND` (Git runs it through `sh`, and ssh parses `-o` values itself); every other use is a structured argument or an environment value (checked: Git runs `GIT_ASKPASS` directly, so a space is fine there). It was therefore an over-narrow guard, not a policy about spaces.

* The directory above the workspace may now contain ordinary characters (spaces, non-ASCII, apostrophes). `ssh_command` quotes the two paths it embeds (shell single quotes; the `UserKnownHostsFile` value also in ssh double quotes) and leaves ordinary paths byte-identical to before.
* Still refused: control characters, `"`, `\\`, `$` and `%` (ssh option parsing and expansion), non-UTF-8 paths. The nonce is now checked on its own: non-empty, `[A-Za-z0-9_.-]`, not `..` (it was previously covered only by the whole-path whitelist, which let `/` through). Validation now happens before any directory is created.
* Unchanged: the 0700 workspace created with `mkdir` semantics (an existing name or symlink fails), owner-pid scoping and sweeping, cleared Git environment, hooks/helpers disabled.
* Tests: real Git poll and checkout (branch and exact commit) under a spaced path with an apostrophe; the real `ssh -G` parsing the generated command through `sh -c`; `GIT_ASKPASS` run directly from a spaced workspace; unsafe names and paths refused and nothing created; a CLI test polling (`watch --once --queue-only`, shipping gate) from a spaced runtime directory with no Docker request. The four runner tests and the CLI test fail on the previous code.
* Not covered: the docker build path from a spaced checkout (live, gate closed); SSH to a real server.

**UI acceptance is still not done.** The F1–F8 run had no screen-control or accessibility interface; only backend behavior (the shipping CLI, same library and gate) was observed. No result in this report or in the validation notes is a desktop-UI validation.

## 11. E.3 — local-folder file URLs and the empty journal directory

Manual UI validation (F2) registered `/Users/juancastro/Harpoon UI Test` and hit two defects.

1. **Raw path in a `file://` URL.** The dialog built `file://${path}`; `validate_url` correctly refuses whitespace. Now the dialog percent-encodes every UTF-8 byte outside `A-Za-z0-9/._~-` (`fileUrlFor`), and the engine's `file://` branch validates the **decoded** path (`file_url_path`): one decoding only (Git decodes a file URL once, and the URL is handed to Git unchanged, so check and execution see the same bytes); a malformed or truncated `%`-escape, non-UTF-8, any decoded control character (NUL, newline, DEL), an encoded `/`, and any `.` or `..` component are refused; `%25` yields a literal `%` that is never decoded again. This also closes a latent bypass: `file:///x/%2e%2e/y` used to pass the raw `..` check and Git would have resolved it. The raw-string checks (no whitespace/control, no `..` substring, absolute path) are kept. Recorded file URLs are shown as folder paths in the UI.
2. **Empty journal directory shown as damage.** `prepare()` creates the journal directory before every operation, so a refused first registration left it empty and `snapshot()` treated "directory exists" as "store exists", which made `SqliteJournal::read_only` fall back to a legacy `journal.wal` that was never there. A directory with neither `deployments.sqlite3` nor `journal.wal` is now `missing`; a directory with either file that cannot be read is still `error` (fail closed).

Tests (each fails on the previous code): `validate_url` accept/refuse tables; real Git polling and checkout through an encoded URL in a folder named `Harpoon UI Test & café`; encoded traversal, NUL, newline, malformed escapes refused before Git runs; CLI: raw URL refused with no store created, bad encodings refused, encoded URL registers, lists and polls; Tauri: empty directory is `missing`, a failed first registration still reads `missing` and a later encoded registration reads `ok`, unreadable SQLite or legacy files are `error`; TypeScript encoder/decoder tables. Not changed: the gate, authorization, Docker, recovery. Desktop UI acceptance has still not been done by automation.

## 12. UI stabilization pass (manual acceptance findings)

Eight findings from manual UI acceptance, each checked against the code first.

| # | Finding | Status | What changed |
|---|---|---|---|
| 1 | Dialog dropdown closes after 1–2 s | **Fixed, verified** (real React + jsdom) | `Dialog` re-ran its "focus the first field" effect on every render because it depended on `onClose`, a new function each render, and the deployment snapshot re-renders the open dialog every 3 s. `useDialogFocus` focuses once on mount and reads `onClose` through a ref. Tests re-render a real dialog 12 times: focus, caret and typed text unchanged, the select is never blurred (a blur is what closes a native dropdown), DOM nodes are not replaced, Escape calls the latest `onClose` once, no listener is left behind. A control test shows the previous implementation fails them. The native popup itself is not testable without a real browser; its closing event (blur) is. |
| 2 | Held Git events hard to find and act on | **Fixed, verified** (Rust, real journals) | A "held Git events" banner on the application and a "Review held events" button in the Automation panel, shown whether or not triggers are paused (previously only a paused application had a way in). The dialog lists commit (short and full), status, source and time, and offers the engine's own decisions: dismiss all (`resume --dismiss`, which also lifts a pause) and select this revision (`resume --revision`). Neither deploys anything and neither touches the gate. Tests: queued events carry everything the dialog shows; dismissal and selection are recorded in the journal and identical on an independent re-read (what a restarted desktop does); an unknown revision is refused and changes nothing. The same dismissal was also run on a copy of the manual-test profile, with the original verified unchanged. |
| 3 | Sticky runtime header overlaps the gate banner | **Fixed, not visually verified** | The sticky `.shell-header` had a 92%-alpha background, so banners scrolling beneath it showed through. It is now opaque with a soft shadow. Judged from the CSS; there was no screen access to confirm it by eye. |
| 4 | Contradictory attended vs automatic wording | **Fixed, verified** (strings checked) | The desktop never polls in the background; it looks for new commits only when "Check for new commits" is pressed. The Automation panel, settings, create dialog, pause banners and rollback plan now say that, call the pill "Manual" instead of "Active", and say "Git triggers" instead of "Automatic deployments". |
| 5 | No feedback when a new commit is recorded | **Fixed, verified** (logic) | After a manual check the result banner says what it found ("1 new commit recorded as held event. Nothing was deployed.", "Starting point recorded…", "No new commits…"). Held events that appear between polls (for example from another process) raise a non-modal info banner with Review / Dismiss, announced once; opening the app does not announce events that were already there. The banner does not take focus. |
| 6 | Reserved `HARPOON_` variable names | **Fixed, verified** (rule tables) | The Add variable form shows the rule under the name and refuses, before saving, names that the engine would refuse: `HARPOON_` prefix, a leading digit, characters other than letters/digits/underscore, over 128 bytes; Config values over 4096 bytes or with control characters. The engine remains authoritative. |
| 7 | Native folder picker | **Implemented, not exercised interactively** | A "Choose…" button uses the same fixed-AppleScript macOS picker the Compose import already ships (`deploy_pick_folder`, no input interpolated into the script). The path only fills the form; it is encoded and validated as before and the engine validates it on registration. The answer parser is unit-tested; the dialog itself needs a person. |
| 8 | Green indicator beside NO RELEASE | **Fixed, verified** | An application with no release fell through to the green check badge. Only something actually running is green now (`heroClassFor`); "No release" and "unknown" use the neutral badge and icon. |

Tests added: `dialogFocus.test.mjs` (6, jsdom), logic tests in `deployEngine.test.mjs` (names, values, announcements, poll summaries, repository names, badge mapping), an integration assertion, and 5 Tauri tests (held events, decisions, persistence, the picker's parser). `jsdom` is a new devDependency (test-only); the lockfile change is additions only.

**Next manual retest.** Restart is not needed for the data: it is read from the journal. In Deployments → your application: (a) Add variable, open "Used in" and wait 10 s: it stays open; type in Value for 10 s: focus stays; (b) try the name `HARPOON_X`: it is explained and Save is disabled; (c) the held commit is shown with a banner and "Review held events", with commit, status, source and time; do not choose Dismiss if you want to keep it; (d) press Check for new commits after adding a commit to the test repository: the result says what was found, and a notice appears for a commit found some other way; (e) scroll with the gate banner showing: the header no longer lets it show through; (f) an application with no release shows a grey badge; (g) Add repository → Local folder → Choose…; (h) the Automation panel says "Manual", with no "Active" or "runs while Harpoon is open".

## 13. External runtime viewer (read-only)

Found when the successful B7 live-acceptance deployment (`dep_01M6DPQ543ASWBHW7PW17ZAJ3C`) could not be seen in the desktop: the
desktop reads only its own store (`<app data>/deploy`), and the B7 runtime is a different directory written by the experimental
B7 build (`236ccec`, one local-only commit on a pre-B9 main) in the legacy `journal.wal` format. The same engine lineage, older.

**What it does.** "Open runtime…" (Projects) picks a runtime folder; `deploy_snapshot_at(path)` validates it and returns the same
snapshot the adapter always produced, tagged `source.kind = "external"`, `readOnly = true`, with its journal format. The
applications appear in their own group, labelled **External · read-only**. Harpoon's own store, its alerts and the sidebar badge
are unaffected. Attachments last for the session; nothing about them is saved.

**Read-only, enforced in the backend.**
* `ExternalRuntime::attach` is the only way to get one. The path must be absolute, free of `..` and control characters, a real
  folder that is not Harpoon's own store, with a real (not symlinked) `deploy-journal` directory holding a regular
  `deployments.sqlite3` or `journal.wal`.
* Its only capability is `snapshot`, which uses `SqliteJournal::read_only` and `writer_lock_status` and nothing else: no writer is
  opened, nothing is locked, created, repaired or migrated. A legacy journal is decoded where it is.
* An operation names no runtime (it always acts on Harpoon's own store); `DeployOp` rejects unknown fields (`recover` became a
  struct variant so that applies to it as well), so a path cannot be smuggled in. In addition, every attached root is remembered
  and `run_op_with` refuses any target that is, contains or lies inside one, before creating or opening anything.
* The frontend repeats this (no mutating dialog opens, controls are disabled with the reason, ops naming an external application
  are not sent), but the backend does not depend on it.

**Recorded vs observed.** The snapshot now carries each deployment's ledger (images, containers, port leases, with ids and
states). For an application recorded as serving, the adapter compares the record with `docker ps`: container id, the three
ownership labels (`io.harpoon.managed`, `.app`, `.deployment`), running state and published ports. The result (`match`,
`unverified` when Docker gave no labels, `stopped`, `absent`, `mismatch`, `unknown`) is shown beside the record; an application is
treated as running only for `match` or `unverified`, and a mismatch is explained instead of being shown as current. This applies to
Harpoon's own applications too. When a journal pinned no release configuration (anything before milestone C), the loopback URL
comes from the recorded port lease and says so.

**Tests.** The B7 journal is a byte-identical fixture (`src-tauri/tests/fixtures/b7-legacy-runtime`; it holds only ids,
timestamps and statuses). Backend: decoding, no write and no migration (tree, bytes and timestamps unchanged; also from a
read-only folder; also for a SQLite runtime), every rejected path form, operations refused for every way of naming an attached
runtime, unknown fields rejected, and a golden file (`b7-legacy-snapshot.json`) of the exact JSON. Frontend: the adapter and the
comparison matrix on that golden file, and the real components in jsdom (workflow, badges, disabled actions, panel, deployment
screen). The golden file is regenerated with `UPDATE_GOLDEN=1 cargo test`.

**Preserved evidence.** The B7 session was copied byte for byte (1032 files, hashes, `diff -r` and permissions identical) to
`~/harpoon-b7-evidence-PRESERVED-20261102` (read-only, with a manifest) before any work; the original in `/tmp` is untouched.

**Not done / limits.** Attachments are per session. Pre-C journals have no pinned configuration, repository, secrets or recorded
step logs; the page says "no repository registered" and offers no build log. Observation is polled, not pushed. A different
Docker than Harpoon's own is not consulted.

### Noted, not implemented (reported during the B7 manual validation)

* **Container-card overflow.** In the Containers view, a card's content can overflow its bounds (long names, many ports or
  labels). Reported from screenshots; not investigated or changed here.
* **Effective port exposure display.** Show what is reachable (binding address and host port against the container port, loopback
  against all interfaces) rather than a recorded or requested port. This change shows the recorded lease beside Docker's published
  ports and reports disagreement, but does not build a reachability view.
* **`*.harpoon.localhost` reverse proxy and DNS.** Friendly per-deployment names (`app.harpoon.localhost`) need a local reverse
  proxy, resolution of `*.localhost` on macOS and a routing and TLS story. This is routing work beyond Milestone E (the roadmap
  keeps zero-downtime routing out of the first release) and is unstarted.
