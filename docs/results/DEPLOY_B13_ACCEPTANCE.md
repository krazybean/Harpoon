# B13 — Runtime configuration, failure hardening and the Milestone B acceptance matrix

Status: offline implementation and audit. **Nothing here was run live; the shipping gate is closed and unchanged;
Milestone B is not accepted.** Live evidence cited below is from L1–L4 only
(`DEPLOY_MILESTONE_B_LIVE_RESULTS_L1_L3.md`, `DEPLOY_MILESTONE_B_L4_RESULTS.md`).

## 1. What B13 adds

### 1.1 Non-secret runtime variables (`--env KEY=VALUE`, criterion 1)

- `plan`, `deploy` and `replace` take a repeatable `--env KEY=VALUE` (the argument parser repeats only `env`; every other
  value flag still rejects a repeat). `plan` validates it read-only.
- **These are not secrets.** The values reach the container, so `docker inspect` shows them, and they are pinned in the
  deployment snapshot, so anyone who can read the journal can read them. Harpoon's own output prints **names only**
  (`environment (…): A, B — NON-SECRET values: …`). Keychain and redaction belong to Milestone C and are not started.
- Validation (before any Docker call or journal write; exit 3): names `[A-Za-z_][A-Za-z0-9_]*` up to 128 bytes;
  `HARPOON_*` reserved (this covers `HARPOON_DEPLOYMENT_MARKER`, with a specific message); duplicates, a missing `=`, control
  characters (including NUL, newline, tab, DEL) and values over 4096 bytes refused; at most 64 variables and 32 KiB. Values
  keep any `=`, and `KEY=` is an explicit empty value. The backend validates again at container creation, whatever the
  source (this run or a pin read from the journal), so a reserved or malformed value never reaches Docker.
- Both the preliminary and the serving container get the variables; each gets its **own** freshly generated marker
  (asserted by exact `Env` arrays from the fake Docker's create requests: three containers, three distinct markers).
- Each release is pinned with its environment. **A restoration reuses the restored release's own pin; a rollback reuses the
  target's pin; neither uses the current command line or another release's values** (tests: failed replacement with a
  different `--env`, failed rollback restoring the serving release, successful rollback).
- `replace` without `--env` gives the candidate no variables; when that differs from the serving release's pin the plan
  says so (names only).

### 1.2 Pin format and SQLite compatibility

- `ReleaseConfig.env` (sorted by name). A release with no environment encodes **exactly as before** (`v1`); one with
  variables adds a trailing `env=` field and is `v2` (each `KEY=VALUE` hex-encoded, comma-separated, canonical order,
  validated on decode).
- Evidence: a pin copied from a real L4 journal decodes and re-encodes byte-for-byte; a SQLite journal holding one `v2`
  and one `v1` pin round-trips through reopen (read-only and writer) and the file contains both `v2;img=` and `v1;img=`;
  flipping one byte inside the stored environment makes the checksummed record fail and the store refuses to open (`status`
  and `recover` exit 4 with "Do not delete"); decode rejects a `v1` with an env field, an empty or out-of-order `v2`
  environment, a reserved name, and trailing fields. No schema migration; `schema_migrations` is untouched.
- **Forward compatibility:** a build from before B13 reading a `v2` pin fails closed ("unknown pinned-configuration
  version") rather than dropping the variables. A journal that has never held a variable stays readable by older builds.
- Old releases (no pin, or a `v1` pin) have well-defined behavior: an empty environment, and a release with no pin at all
  is still `not_attempted (config_unavailable)` for restoration, as before.

### 1.3 Preliminary failure (criterion 3)

- Engine behavior was already correct and is now covered end to end: a failed source, build or preliminary check leaves the
  previous release running (no stop/restart/kill request names it), begins no cutover, records the failing step, keeps the
  candidate's image (owned and in the ledger), removes only the step-scoped preliminary container, attempts no restoration
  and does not pause triggers (no `Pause` in the report, none derived from the journal).
- New: the CLI prints what happened and **only real commands** (`src/next_steps.rs`): the failing step, whether a release
  keeps serving, "no restoration was attempted", "not paused", the retained image, then `status` (inspect), the deliberate
  retry (`deploy`/`replace`/`rollback` again, a new deployment and a new image) and `cleanup-image … --deployment … --image …`
  (a dry run unless `--apply`). Bounded log lines and health evidence were already printed. A test runs the printed
  `cleanup-image` command and checks that every `harpoon-deploy <word>` in the output is a real command.
- Coverage: failed build; candidate never healthy (connection refused and 503); candidate exits during the check;
  a Docker API outage at image inspect, create and start; each with a second application's container untouched.
  CLI-level: live `deploy` with the fake Docker for never-healthy and failed build, status after restart, a deliberate retry.
  There is no `logs` or `retry` command and none is advertised.

### 1.4 Status and CLI corrections

- "its candidate never served" → "was never confirmed as the successful serving release (it may have answered a health
  check before the final confirmation)" (L2 showed a failed candidate answering).
- The restoration wording now depends on how it happened: an `interrupted` deployment "was restored by an explicit
  restoration"; a `failed` one "by the automatic bounded recovery of that run".
- One observation of the writer lock per `status` invocation, shared by the header and every note.
- `releases` no longer advertises `--simulate` for a live runtime; it says the build cannot roll back a live runtime.
- Forwarder default `--forwarder-command` is now `harpoon`, the real process name recorded in
  `DEPLOY_B7_LIVE_PREFLIGHT.md` (the old `Harpoon` failed on a correct listener, as L1 found). The comparison is still exact,
  case-sensitive and 9-character truncated, and the pid must still match; an explicit value is used verbatim.

## 2. Offline failure matrix — audit

Existing coverage was inspected first and not duplicated. Where a row says *existing*, the named tests predate B13.

| Area | Status | Tests (crate) |
|---|---|---|
| `restored` | existing | `a_failed_cutover_restores_the_previous_release_from_its_pinned_configuration` (docker) |
| `restored_unverified` (health inconclusive) | existing | `a_restored_release_that_never_passes_its_check_is_reported_unverified` |
| `restore_failed` (start failure), not retried | existing | `a_restoration_that_cannot_start_the_container_is_restore_failed_and_not_retried` |
| `not_attempted`: image missing, Docker unreachable, no pin | existing | `a_missing_previous_image_or_unreachable_docker_is_not_attempted`, `a_missing_pinned_configuration_is_not_attempted_and_creates_nothing` |
| Restoration timeout | existing | `the_restoration_deadline_is_enforced` |
| Port blockers (automatic and explicit, attempt not spent) | existing | `an_occupied_port_*`, `a_running_container_on_the_port_blocks_*` |
| Crash mid-restoration, every write | existing | `a_crash_at_every_write_never_duplicates_the_restoration_*`, `a_crash_during_the_explicit_recovery_*` (engine, docker, journal) |
| One bounded attempt, never repeated, other apps untouched | existing | b11/b12 engine, docker and CLI suites (bystanders asserted) |
| Application port occupied / foreign owner / collision | existing | `a_port_collision_*`, `a_port_taken_between_the_check_and_the_start_*`, `a_stopped_containers_unreadable_port_bindings_*` |
| Image build failure; disk-space preflight | existing | `a_failed_build_*`, `an_oversized_context_or_low_disk_stops_before_any_build` |
| Docker API unavailable | partly existing | restoration and inventory cases existed; **new:** outage at image inspect/create/start during preparation |
| Preliminary failure with a serving release | partly existing | **new:** never-healthy, exits during check, build failure, invariants (docker); CLI next actions |
| Concurrent writers, one application | partly existing | `a_second_writer_is_refused_at_once_*`; **new:** every mutating command refused while a writer holds the journal; second same-application deploy refused |
| Crash at state transitions | existing | the `a_crash_at_every_write_*` family |
| Environment variants of the above | **new** | restoration, rollback and failed-rollback environment tests; reserved/malformed variables never reach Docker |

## 3. Pause and resume contract audit (D4/D5, offline requirement)

What exists: a pause is **derived** from deployment records (`trigger_pauses`): an open or failed cutover, or a successful
rollback, pauses the application; the reason (the deployment, the failure, the recovery outcome or the rollback target) is
shown by `status`; an ordinary build failure or failed preliminary check does not pause (tested). Deployment status
`superseded` and `AppQueue` (a newer queued request supersedes an older queued one and reports it) exist in the engine.

What does **not** exist: a stored pause record with its own timestamp (the originating deployment's times are the only
time); any trigger source, so no `held` event record (source revision, delivery id); `resume` (deploy newest / chosen /
dismiss); dismissal records; a CLI path that records `superseded` requests. `status` itself says "there is no `resume` yet".

Assessment: the acceptance text lists held events and an explicit `resume` under Milestone B's failure tests, but Milestone B
has no trigger source (Git, polling and webhooks are Milestone D), so nothing can arrive while paused. I have **not**
declared this satisfied. The smallest implementation, as a separate follow-up (B14, not started):
1. a durable `held` record kind in the journal (application, source revision, delivery id, received time) and a durable
   `pause` record with reason and time; 2. `status` lists both; 3. `resume --app A` with exactly one of `--deploy-newest`,
`--deploy REVISION`, `--dismiss`, each recording its decision, with `dismissed`/`superseded` states; 4. a single offline
test-only command or library call to record a held event, since no real trigger exists. No scheduler, Git or webhook.

## 4. Milestone B acceptance matrix

Classes: **Live** passed live · **Offline** passed offline only · **Impl.** implemented but unverified · **Failed** ·
**Blocked** · **None** not implemented. Offline results are not upgraded to live.

| # | Criterion | Class | Evidence and gap |
|---|---|---|---|
| 1 | Build, run, non-secret variables, preliminary check before replacement | **Live** for build/preliminary/start (L1–L4); variables **Offline** | `--env` pinned and reproduced (offline, B13). No live run has passed variables to a real container. |
| 2 | Replacement reports interruption beforehand, records measured duration | **Live** | L1 (717 ms observed, 1335 ms recorded), L3, L4 |
| 3 | Preliminary failure: nothing stopped, retained failed release, actions listed | **Offline** | B13 tests and CLI output; no live preliminary failure with a running release was run |
| 4 | Bounded restoration and its four outcomes | `restored`: **Live** (L2 injected, L4 explicit); `restored_unverified`, `restore_failed`, `not_attempted`: **Offline** | no natural failing application live; no live `restored_unverified`/`restore_failed`/`not_attempted` |
| 5 | Rollback with pinned configuration, data/schema warning | **Live** | L3 (pinned image id, no build, warning printed) |
| 6 | History survives restart and `kill -9`; interrupted shown; bounded recovery offered | **Live** for one injected crash boundary (L4) plus restart survival (L1–L3); other crash points **Offline** | L4 is one boundary, not exhaustive |
| 7 | Deleting the store leaves workloads; `status` reports labelled resources | **Offline** | `status --docker-socket` (PR #130); no store deleted live |

| Failure test | Class |
|---|---|
| Docker unreachable | **Offline** (inventory, restoration `not_attempted`, preparation outage); not run live |
| VM stopped mid-deploy | **None live**: forbidden by the acceptance safety rules so far; needs a specific authorization |
| Port already in use | **Offline** |
| Health check never succeeds | **Offline** (B13); **Live** only as the injected final-check fault (L2) |
| Image build fails | **Offline** |
| Disk full during build | **Offline** (preflight); not live |
| Two concurrent deploys of one application | **Offline** (writer lock, same-application refusal) |
| Deploy of A while B runs (B untouched) | **Live** (L2 bystander, L4 bystander 1188/1188); **Offline** broadly |
| Restoration failures (cannot start, image/pin unavailable, Docker unreachable, over budget) | **Offline** |
| Engine killed mid-cutover | **Live** (L4, one boundary); mid-restoration **Offline** |
| Pause: build/preliminary failure do not pause; failed cutover and rollback do; reason in `status` | **Offline**; failed cutover and rollback pause seen live in `status` (L2, L3) |
| Events held while paused; explicit `resume` | **None** (see 3) |

## 5. Stop conditions

| Condition | Evaluation |
|---|---|
| Healthy release stopped before the preliminary check passed or before the plan was shown | Not triggered: **Live** order checked in L1–L4 (preliminary finished before the stop); **Offline** across the matrix |
| One bounded restoration; no unrecorded or unbounded restoration | Not triggered: **Live** (L2, L4); second attempt refused (**Live** L4) |
| `restored` only after the previous release started and was checked | Not triggered: **Live** (real marker-attributed health in L2, L4) |
| No automatic rollback or retry | Not triggered: **Offline** (never-retry tests); no automatic action seen live |
| Unrecoverable state after crashes at each transition | Not triggered **Offline** (crash-at-every-write suites); **Live** only one boundary |
| No resident process | Holds by construction: the CLI is one-shot (no threads, daemon or loop in `deploy-cli/src`); `ps` showed no `harpoon-deploy` process after runs. **Offline** observation, not a long-running measurement |
| Idle RSS ≤ 30 MiB, idle CPU ≤ 0.5 % | **Not measurable as stated**: there is no idle process. Proxy measured locally and offline on this machine: `harpoon-deploy status` on an empty runtime, release build, peak RSS 2.2 MB, 0.00 s user+sys (3 runs). This is a transient proxy, not an idle measurement |
| Dockerfile build overhead ≤ 1.2× a direct build | **Pending.** Not measurable from existing evidence: `DEPLOY_BASELINE.md` could not run a like-for-like guest comparison, and L1–L4 builds used different contexts and caches. No number is claimed |

## 6. Pending live procedures (not run; each needs its own authorization)

**P1 — build overhead (bounded).** One small Dockerfile context, one builder (the bundled Buildx, `--no-cache`, same
base image already present), alternating *direct* (`docker-buildx build` through the socket) and *engine* (`deploy` up to
its build step) runs, N=5 each after one discarded warm-up, host wall clock; compare medians; record the engine's own
`build` step duration and the extra steps separately (source confinement, tag verification). Pass: median ratio ≤ 1.2.
Cleanup: remove only the images the runs created, by id.

**P2 — idle/resident.** Immediately after a `deploy`, `replace` and `status`, `ps` for any `harpoon-deploy` process and any
child; none expected. Pass: none for 60 s. (There is no idle RSS to sample if none exists.)

**L5 — runtime variables live.** One application, `--env` with an empty value and a value containing `=`; `docker inspect`
of the preliminary-turned-serving container shows exactly the array plus its own marker; replace with different values;
then a rollback and an explicit restoration (after an injected failure) each show their own release's pin; journal copy shows `v2`.

**L6 — preliminary failure live.** A running release and a candidate that fails its preliminary check (a Dockerfile whose
app does not answer `/health`); confirm the serving container's `StartedAt` and id are unchanged, no pause in `status`, the
printed next actions, and `cleanup-image` dry run before apply.

**L7 — criterion 7 live.** With a running application and a bystander, move the runtime directory aside (never delete),
run `status --docker-socket`, confirm the labelled containers are listed with the missing-store warning and keep running,
then restore the directory and compare the journal hash.

## 7. Remaining gaps before acceptance

1. Live: L5, L6, L7, P1, P2 (and any decision on the "VM stopped mid-deploy" and disk-full tests).
2. Criterion 4 outcomes other than `restored`, live, ideally with a naturally failing application.
3. Held events and `resume` (B14), or a recorded decision that they move to the milestone that introduces triggers.
4. Criterion 6 beyond one injected boundary (for example a crash mid-restoration live), if the owner wants more than L4.
5. The experimental live wiring (replace, rollback, restore, fault injection) exists only on local, unpushed branches; shipping
   `replace`/`rollback`/`restore` stay simulate-only. A decision is needed on how the acceptance-proven wiring becomes a
   reviewed, gated change before Milestone B can close.

## 8. Newly discovered defects

- None in the engine. CLI: the forwarder default and three wording defects (all fixed here). Observation: `status` says a
  release is "serving" from the records even when its container was removed outside Harpoon; that is by design (records only)
  and is now qualified only for unfinished cutovers.
