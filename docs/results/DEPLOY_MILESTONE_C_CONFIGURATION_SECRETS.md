# Milestone C: configuration, builds and secrets (implementation and evidence)

Status: **implemented offline as one vertical slice; not live-accepted.** Everything below was verified against a fake
Docker, a fake Buildx and an in-memory secret store, plus one opt-in round trip on the real macOS Keychain using a
disposable item. No live Docker, VM, Buildx or `docker history` evidence exists; that is listed as a gap, not claimed.
`Gate::shipping()` is unchanged (the live lifecycle stays closed in the shipping binary). Milestone B was not reopened.

## 1. Ponytail review (done first, built into the slice)

| | |
|---|---|
| **KEEP** (essential to C) | Scoped resolution with explicit unset; Config/Secret types and build/runtime flags; immutable snapshots; Keychain items per version; secret files for BuildKit; stream redaction; pin extension; a diff that shows no values. |
| **REUSE** (B already had it) | The SQLite journal and its framed, checksummed records (B14's pattern: one new record family, replayed fail-closed); `ReleaseConfig` and its versioned codec; `envvars` validation and the `HARPOON_*` reservation; `RunConfig.redactor` and the engine's per-step `LogSink`; Buildx isolation, source confinement, the budget/kill logic in `run_build`; `--context` and `--dockerfile` overrides; `rollback`/`restore` already take the pinned configuration; the fake Docker/Buildx harnesses; the flag-only argument parser. |
| **CUT** | No new CLI framework (new commands are plain `Spec`s); no credential service or vault abstraction beyond a three-method trait; no general config framework; no branch scope; no per-key ACL; no hashes of secrets (D6 forbids them); no echo-suppression terminal code (stdin must be a pipe or redirect); no image-reuse machinery (below). |
| **DEFER** | Branch overrides (preview environments, Milestone E); image reuse for runtime-only changes; Gate U (headless/locked keychain behaviour); any event/Git behaviour (D); UI (E). |
| **RISK** (blockers found and handled) | (1) The old `Redactor::apply` replaced secrets one after another, so a one-character secret could corrupt an earlier marker and a nested secret could leave a tail: replaced by a single-pass scanner. (2) `Redactor` and `LogSink` derived `Debug` and would print secrets and held-back raw text: manual redacted `Debug`. (3) `run_build` truncated an over-long line *before* redaction, which could leave a secret's prefix visible: output is now streamed in chunks and redacted before any line is stored. (4) A withheld capture was silently dropped by the engine (`lines_seen == 0`), so the record did not say output was withheld: fixed. (5) Any pin carrying a snapshot id would have become `v3` and changed the legacy bytes: a run with only `--env` still writes `v1`/`v2`. (6) A restore or replacement could stop the serving release and only then find a secret missing: secrets are fetched, for the new release **and** for the release that might have to be restored, before anything is built, written or stopped. |

## 2. What a developer can do now

```
harpoon-deploy config-set  --scope project:shop@prod --name LOG_LEVEL --value debug              # Config, runtime
harpoon-deploy config-set  --scope project:shop --name NODE_ENV --value production --build       # Config, build argument
printf %s "$TOKEN" | harpoon-deploy secret-set --scope app:APP@prod --name DB_PASSWORD           # Secret, runtime; again = rotation (version N+1)
printf %s "$NPM"   | harpoon-deploy secret-set --scope global --name NPM_TOKEN --build           # Secret, BuildKit mount
harpoon-deploy config-unset --scope app:APP --name LOG_LEVEL                                      # blocks inheritance
harpoon-deploy config-defs                                                                        # definitions (secrets as id@version)
harpoon-deploy config-show --app APP --project shop --environment prod [--check-secrets]         # effective config + source scope
harpoon-deploy config-show --deployment DEP                                                       # a release's immutable snapshot
harpoon-deploy config-diff --from DEP [--to DEP]                                                  # names, scopes, flags, versions only
harpoon-deploy deploy|replace --project shop --environment prod --target release --build-arg K=V ...
```

* **Scopes** (`--scope global|project:NAME|app:APP_ID`, each with an optional `@ENV`), most specific wins:
  application+env > application > project+env > project > global+env > global; an explicit unset blocks inheritance;
  the latest definition at a scope wins. `--env` and `--build-arg` on the command line sit above every scope.
* **Types and flags.** Config is readable (`config-show`, `config-defs`); a Secret is only ever `id@version`.
  Each variable is `--build`, `--runtime` or both (default runtime). Runtime values reach the container; Config build
  values are `--build-arg`; build Secrets are BuildKit `--secret` mounts, never an argument.
* **Snapshots.** Every deployment writes one immutable snapshot (key, source scope, type, flags, secret version)
  before the build. It is never recomputed; `config-show --deployment` and `config-diff` read it.
* **Releases.** A release is pinned with its runtime secret references (exact versions), the snapshot it came from and
  a canonical build description. `restore` and `rollback` use the pin of the release they recreate, never current values.

## 3. Reused versus new

| Reused unchanged or extended | New |
|---|---|
| SQLite journal, framing, replay, fail-closed checks, `Journal` trait (one default method added), `Restored` (one field) | `deploy/src/config.rs` (scopes, definitions, snapshots, resolution, diff, codec, replay rules); `deploy/src/secrets.rs` (reference, value type, store trait, in-memory fake); `deploy-keychain` crate (Security.framework adapter); `deploy-cli/src/config_cmds.rs` |
| `ReleaseConfig` codec (v1/v2 kept; v3 added) | `SecretFiles` (build secret files and sweep) in `buildx_run.rs` |
| `LogSink`/`Redactor` (rewritten internals, same API plus `push_chunk`, `failing`) | the stream scanner |
| Backend build/container creation (env, buildx request) | `DockerConfig` build fields, `with_secrets`, effective secrets |
| Existing flags `--context`, `--dockerfile`, `--env` | `--target`, `--build-arg`, `--project`, `--environment` |

## 4. Keychain and security design

* **Store.** Service `io.harpoon.deploy`, account `<secret-id>@<version>`, one generic-password item per version, written
  once (`errSecDuplicateItem` is surfaced; nothing overwrites). The secret id is `<scope>/<NAME>`, stable per scope and
  name, so a re-save continues the version numbers. SQLite holds references, scopes, flags and names; never values or hashes.
* **Adapter.** `deploy-keychain` calls Security.framework directly (`SecItemAdd/CopyMatching/Delete`) with the value as
  `CFData`: no process, no `security` CLI, nothing on a command line. Every query sets `kSecUseAuthenticationUI` to fail, so a
  locked keychain or an access prompt becomes an error naming the reference; protection is not weakened to avoid a dialog.
  Off macOS the adapter reports `Unavailable`; there is no plaintext fallback. The trait lives in the engine crate with an
  injectable fake (`MemorySecretStore`, with locked/denied/unavailable modes).
* **Input.** `secret-set` reads the value from standard input only; an interactive terminal is refused (it would echo);
  there is no `--value` for secrets; one trailing line break is dropped; empty, NUL, non-UTF-8 and over-16 KiB values are
  refused without echoing them. A value under 8 characters is accepted with a warning about over-masking.
* **Ordering.** The Keychain write comes first, so no record names an unsaved item; if the record then cannot be written
  the item is deleted. A crash between the two leaves an orphan item; the next save skips to the next free version
  (versions only increase, they need not be consecutive).
* **Fail before side effects.** `deploy`, `replace`, `rollback` and `restore` resolve and **fetch every secret they could use**
  (the new release's, the target's, and the serving release's pinned secrets because a failed cutover restores it) before
  the journal is written, anything is built or any container is touched. A missing, locked or denied item stops the command
  with the key and `id@version` named, exit 6. The backend re-checks (`plan_release`, `restore_blockers`, container creation)
  as defense in depth. Read-only commands (`status`, `config-defs`, `config-show`, `events`) need only references and work
  with the Keychain locked; `--check-secrets` probes availability and exits 6 if any is unavailable.
* **Build secrets.** Written as `0600` files in a `0700` directory `scratch/buildsecrets-<nonce>`, beside (never inside) the
  context snapshot, passed as `--secret id=NAME,src=FILE`; the directory is removed on drop (success, failure, timeout and
  unwinding). A hard kill of the process skips `Drop`: the next build sweeps any `buildsecrets-*` directory first (one writer at
  a time). **Limitation:** between a hard kill and the next build, plaintext build secrets remain in that `0700` directory under
  the runtime directory. The engine does not cancel a running Buildx mid-step; cancellation applies between steps.
* **Values in memory.** `SecretValue` has no `Display`, a redacted `Debug`, and is zeroed on drop (best effort; copies made by
  the allocator, Security.framework or the OS are not tracked). `SecretBundle`, `Redactor` and `LogSink` also format without values.

## 5. Redaction

Single pass over the stream: every occurrence of every secret is found, overlapping and nested matches merge into one span
(`[REDACTED:NAME]`, naming the longer or both overlapping keys), no minimum length, a marker is never re-scanned. The end of each
chunk that could start a secret is held back until the next chunk or the end of the stream, so splits at any byte, multi-line
values and one-byte-at-a-time delivery are masked. Redaction runs before a line is stored. If the redaction step fails,
the raw text is withheld, the capture says so (`withheld`), and the step's record keeps it.

Not claimed (documented limits, not defects): **transformed secrets are not detected** (base64, URL/JSON escaping, hashes, case
changes, truncation, a value an application splits across records); Harpoon masks exact known values only. A Dockerfile that
prints or copies a secret into an image layer is outside Harpoon's control (BuildKit mounts avoid layers, a build step can still
leak). Runtime variables, secrets included, are visible in `docker inspect` of the running container and to processes in it, and
persist in Docker's configuration on the guest disk; Docker's own log store keeps whatever the application writes, and
`docker logs` run directly bypasses Harpoon. A terminal's scrollback or shell history is outside Harpoon. `docker history` and
image-layer exposure were not observed here (no real BuildKit): not live.

## 6. Release snapshot compatibility

* `v1` and `v2` pins decode and re-encode byte-for-byte as before; a run using only `--env` still writes `v1`/`v2`. A release is
  `v3` only when it has runtime secrets, scoped definitions, build configuration or a target
  (`v3;<base>;env=;sec=;snap=;build=`, each possibly empty; names validated, sorted, unique, no overlap between a value and a
  secret). Older builds fail closed on `v3` rather than dropping what they do not understand.
* A store without configuration records reads exactly as before (`ConfigState::default()`). Configuration records replay
  fail-closed: a reference to a secret version never saved, a version that does not increase, a second snapshot for one
  deployment, a pin naming an unrecorded snapshot or an unsaved secret, or a bad checksum refuses the store.
* B14 pauses, held events and resume decisions are untouched (the existing suites, the rollback pause assertion in the
  workflow test).

## 7. Tests (all offline)

| Crate | Passing | New for C |
|---|---|---|
| deploy | 233 | scopes and precedence, unset, secret versions, record codec and damage, diff/display, size limits, secret types and the fake store, v3 pins, redaction (every length, every chunk boundary, multi-line, nested/overlapping, marker integrity, forced failure, Debug) |
| deploy-journal | 46 | config records replay/legacy store/fail-closed/pins/cut after every write/second writer |
| deploy-docker | 227 | Buildx flags, secret files (mode, location, not in context, not in argv or environment), cleanup on success/failure/timeout, stale sweep, missing secret, output masking of every shape and chunking, withheld output, runtime injection, pin holds references |
| deploy-keychain | 1 (+1 ignored) | status mapping; the ignored test is the real round trip |
| deploy-cli | 171 (172 with `--features acceptance`) | the eleven-step workflow, stdin-only input, Keychain locked/denied/unavailable, a restore blocked by an unavailable pinned secret, the leakage matrix through a failing build |

fmt, Clippy `-D warnings` (all five crates; `deploy-cli` also with `--features acceptance`) and the CI source guards are clean.

* **Native Keychain.** `cargo test -- --ignored` in `deploy-keychain` ran once on this Mac against a unique service name and
  account: add, duplicate refused, multi-line read-back, missing item, delete, repeat delete. No dialog appeared (the query
  forbids one) and no existing credential was read or changed. This is one run on one machine with one binary identity; behaviour
  under ad-hoc versus Developer ID signing, a locked login keychain, and a headless session belongs to Gate U.
* **Leakage matrix** (values of 1, 3, 8 and 40 characters, a multi-line PEM-like value, a nested pair, a partly overlapping pair):
  absent from CLI stdout/stderr (including failure evidence), step logs and errors, `config-*` output and diffs, the SQLite
  file and WAL (byte scan of the whole runtime directory), temporary files (removed after success, failure and timeout; stale
  ones swept), and the fake Buildx's argv and environment; present, as documented, in the container's create request (runtime
  secrets). A base64 rendering of a secret printed by the build is **not detected** and the test asserts that, and that this
  document says so.

## 8. Not done, or not proven

* **Image reuse for runtime-only changes: not implemented.** A runtime-only change today goes through `replace`, which rebuilds.
  Doing it safely needs a replacement mode that serves the serving release's pinned image id (the rollback mechanism, aimed at
  the current release) and a rule for when the build identity makes reuse valid. The pin now records the build identity and
  the snapshot, so the data exists; the engine change is a separate, reviewable step.
* **No live evidence.** Real Buildx secret mounts, `docker history` and layer contents, a real container's `inspect`, a real
  Keychain lock during a deploy, and the interplay with the forwarder were not exercised. Live Docker is not authorized here.
* **No branch scope; no external providers; no `--json` output** (there is none in the CLI, so there was nothing to review).
* **Accepted limits:** secret files exist in the runtime scratch directory for the duration of a build and after a hard kill
  until the next build; values are copied in memory by libraries outside this code; redaction is exact-match only.
* **Dockerfile settings** are command-line choices (`--context`, `--dockerfile`, `--target`), recorded in the pin's build
  identity, not scoped variables.

## 9. Recommendation

The slice is complete and consistent offline, with the safety requirements enforced by construction and tested. Milestone C is
**not ready for closure** until a separately authorized live session shows (a) a real BuildKit secret mount absent from
`docker history` and layers, (b) a real container receiving runtime secrets and `docker inspect` exposure matching this document,
(c) the Keychain behaving as designed when locked mid-session and under the shipping binary's signing identity, and (d) a
decision on image reuse for runtime-only changes (implement, or accept a rebuild). After those, it can close, and Gate U can start.
