# Milestone B6 — Gated developer CLI

Status: on `feature/deploy-b6-cli`; **not merged**. Nothing here touched the running VM, a live Docker socket, a real build, or a real
container; the shipping UI and release artifacts are unchanged. B7 is not started.

## What exists
`deploy-cli/` (binary `harpoon-deploy`, no registry dependencies, no CLI framework; arguments parsed by hand) wires the existing engine,
`DockerBackend` and `FileJournal`. `deploy-journal` gained `lock_status()` (inspect the writer lock without writing).

| Command | Writes? | Docker? | What it does |
|---|---|---|---|
| `status` | no | no | deployments (newest first), per-deployment resources with ids/state/keep-policy, unresolved cleanup, outstanding leases, lock state, torn-tail note |
| `doctor` | no | read-only, only with `--docker-socket` | source (confined, Dockerfile, context size), bundled Buildx, journal location/readability/lock, runtime ping, base images present locally |
| `plan` | no | read-only, only with `--docker-socket` | validated config; the five ordered operations with the ports a deploy would pick (journaled leases + Docker mappings); blockers; exit 6 if `deploy` would refuse |
| `recover` | journal | never | marks interrupted deployments (idempotent); lists unresolved items; does not reconcile |
| `reconcile` | dry run: no; `--apply`: journal + Docker | explicit socket | `plan` of the ledger against the runtime; `--apply` runs the engine's `reconcile` (every removal re-verifies id, name and all three labels) |
| `deploy` | journal (+ Docker) | gated | first-deployment workflow; refuses unless every gate passes |
| `export-journal` | new file only | no | copy of the journal bytes to a new `0600` file; works on a corrupt or locked journal |

Exit codes: 0 ok · 2 usage · 3 source/configuration invalid · 4 journal (corrupt, locked, permissions, wrong location) · 5 runtime unreachable or not
configured · 6 refused by a safety gate · 7 deployment failed or work remains.

## Journal and runtime rules (as implemented and tested)
- `--runtime-dir` is required, absolute, never guessed. Journal = `<runtime>/deploy-journal`. Writers create both directories `0700` (parent must exist), and
  `journal.wal`/`journal.lock` are `0600`. An existing directory with group/other access, or a symlinked journal directory, is refused with the `chmod 700` fix.
- A `runtime-kind` marker (`docker` or `simulated`, `0600`) keeps simulated and real journals apart; neither command kind will use the other's directory, and a
  simulation refuses an existing unmarked journal. Simulated runs therefore cannot leave fake leases in a real ledger.
- The ledger is restored before anything is allocated or run (`RunConfig::prior_ledger`), so journaled leases steer allocation; the lock is fail-fast and the
  message says not to create a second journal directory.
- Read-only commands use `read_only()`: no lock, no creation, no truncation, no interruption marking (tested: bytes, mtimes and directory listing unchanged,
  including with a torn tail).
- Corruption fails closed with the damaged line, an `export-journal` hint and "do not delete or reset". Nothing is repaired, skipped or discarded.
- `deploy` refuses (6) while any deployment is unfinished, any ledger entry needs a decision (failed/refused/interrupted cleanup, unreceipted intent, entries a
  finished deployment should have cleaned), or the application already has a recorded container (release replacement is not implemented). Retained images and a
  successful deployment's serving container are not "unresolved".

## Execution safety
- Docker contact requires `--docker-socket` (absolute path). The environment is never read; tests set `DOCKER_HOST`/`DOCKER_CONTEXT` canaries and see zero requests.
- Changing Docker state (`reconcile --apply`, live `deploy`) additionally needs `--authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE`, checked before any I/O.
- Live `deploy` is closed by `Gate::shipping()`, a constant in the binary's call path: no flag, variable or file opens it. Even with a reachable socket and every flag, the shipping binary
  exits 6 before reading, writing or contacting anything (tested in-process and as a real process). Tests open the gate to prove the wiring against the fake Docker only.
- No cleanup, pruning, release replacement or rollback is implicit in any command. `reconcile` never treats an unlistable runtime as "absent" (exit 5, no journal write).

## Tests
`deploy-cli`: 24 tests (in-process against the deploy-docker fake Docker on a temp socket and fake Buildx, plus real-process checks): usage and exit codes; fresh runtime creates nothing for
read-only commands; doctor/plan checks; complete simulated deployment through the real journal with permissions/markers and status afterwards; read-only status; cross-deployment leases and
plan ports; unfinished deployment → refused → `recover` → idempotent; unreceipted creation keeps blocking; corrupt journal on every command + export (no overwrite, no export into the journal);
held lock; loose permissions, symlink, mixed runtime kinds; live gate (shipping and open); gate open → `DockerBackend` + journal drive a failing deployment cleanly (preliminary container removed, nothing
foreign touched, state in `status`); foreign same-name container refused and untouched; owned container removed with `force=false`; unreachable/erroring runtime writes nothing; recorded release blocks a second
first-deployment. Other suites unchanged: `deploy` 135, `deploy-docker` 113, `deploy-journal` 25.
Mutation checks: 12 injected (live gate ignored, authorisation text unchecked, failed/refused cleanup not counted, read-only restore marking interrupted, directory mode unchecked, export into the journal allowed, unlistable runtime treated as empty, unfinished deployments ignored, runtime kinds mixed, recorded release ignored, dry run applying, retained entries counted as unresolved): **12 caught** (two needed test additions after the first pass: the kind-mixing check and the unlistable-runtime check).

## B7 hand-off — first authorized disposable deployment
Prerequisites (none of this has been done): written authorization naming the disposable application; a build of the CLI in which `Gate::shipping()` is deliberately opened in a reviewed B7 change;
the B7 gates below implemented and tested. Then, with a **fresh** runtime directory used for nothing else:

```text
R=<private absolute dir, will be created 0700>      P=<disposable project dir>      N=<container port>
RES=<Harpoon.app/Contents/Resources>                S=<Harpoon's Docker socket, given explicitly>
harpoon-deploy doctor --runtime-dir $R --project-dir $P --container-port $N --resources-dir $RES --docker-socket $S   # must exit 0
harpoon-deploy plan   --runtime-dir $R --project-dir $P --container-port $N --resources-dir $RES --docker-socket $S   # must exit 0, "blockers: none"
harpoon-deploy status --runtime-dir $R                                                                                 # empty / nothing unresolved
harpoon-deploy deploy --runtime-dir $R --project-dir $P --container-port $N --resources-dir $RES --docker-socket $S \
    --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE
harpoon-deploy status --runtime-dir $R        # afterwards; then reconcile (dry run) and review before any --apply
```
Preflight record to keep: output of `doctor` and `plan`; the list of existing containers, images and volumes before the run (to compare afterwards); the journal copy via `export-journal`.

Mandatory B7 gates carried forward (all still open):
1. **Unique build-image identity and tag-race mitigation:** per-attempt random tag recorded in the intent + `--iidfile` identity, remove by ID only (design in `DEPLOY_DOCKER_BACKEND.md`, B4).
2. **Deployment-specific health marker:** `ResponseMarker` exists in the backend but the CLI does not expose it; B7 must generate a per-deployment marker, hand it to the disposable app and require it.
3. **Host-to-VM forwarder ownership verification:** which process owns the host listener for the leased port, and that it forwards to this VM.
4. **Loopback-only host exposure and LAN isolation:** publishing without `HostIp` binds the wildcard on the Docker side; verify what the host actually exposes.
5. **Guest-side disk-space verification:** the preflight measures the host.
6. **Preservation of existing containers, images and volumes:** before/after comparison on the real runtime; anonymous-volume images stay rejected.
7. **Real Docker API response verification:** replace the documentation-derived fake with captured responses.

## Remaining gaps
- The successful `DockerBackend` path is not driven through the CLI in tests (it needs a real HTTP listener behind a published port); only the failing path is.
- No `--json` output and no owner (uid) check on the runtime directory.
- `reconcile` needs `--resources-dir` only because the backend constructor resolves Buildx; it never builds.
- `doctor` and `status` take the writer lock shared/not at all, so a writer starting in that instant can see `Locked` once and retry.
- Everything from the B5 risk list still applies (no power-loss testing, no journal salvage beyond export, one writer per runtime directory).

## B7 phase 1 changes to the CLI
- `deploy` (live path) now sets `live_gates` and per-container attribution and requires `--forwarder-pid N` (optional `--forwarder-command`, default `Harpoon`; `--lsof-path`, default `/usr/sbin/lsof`);
  it attaches the host-listener check and a guest free-space probe. *(phase 1 text, superseded below: the probe now exists)* No guest probe existed, so a live deployment stopped at `source` ("no guest-side free-space probe is implemented") even with the gate open;
  `Gate::guest_free_override` lets tests stand in for it and is `None` in the shipping build.
- New read-only `inventory --docker-socket S [--lsof-path P] [--to FILE]`: sorted, timestamp-free lines for containers, images, volumes, Docker port mappings and (with `--lsof-path`) host listeners in 38000–39099,
  for before/after `diff`s. `--to` creates a new `0600` file and never overwrites.
- The image tag printed by `deploy` is now a pattern; the real tag (with 128 random bits) is chosen at build and recorded in the journal.
- B7 hand-off: replaced by `docs/results/DEPLOY_B7_LIVE_ACCEPTANCE.md`; the invocation there needs the added flags. The seven carried-forward gates: 1 (image identity) and 2 (marker) and the source-swap window are implemented offline; 3 (forwarder ownership) and 4 (loopback/LAN) have an offline verifier and an audit but are unproven live; 5 (guest disk) has a rule and no probe; 6 and 7 are live-only.

## B7 final offline blockers — CLI changes
- **`--mgmt-socket PATH`** (absolute): required by a live `deploy`, optional for `doctor`. Guest free space is measured through Harpoon's existing management channel (host unix socket → vsock 2377 → `harpoon-mgmt`), cross-checked against Docker's `/info`
  (hostname, kernel, data root); the old test-only `Gate::guest_free_override` is gone — tests use a fake guest. `doctor --docker-socket S --mgmt-socket M` reports `guest free space` (exit 5 when short or unverifiable).
- **`teardown --deployment dep_… --docker-socket S --resources-dir RES [--apply --authorize-live-docker-changes=…]`**: dry run by default; dismantles ONE succeeded deployment as described in `DEPLOY_B7_LIVE_ACCEPTANCE.md` §4.
  Exit codes: 0 done/nothing to do, 5 runtime unreachable (no writes), 6 refused (unknown/unsuccessful deployment, earlier refusal, simulated runtime, missing opt-in), 7 incomplete (state recorded).
- Live `deploy` remains closed in the shipping build (`Gate::shipping()`); `teardown` needs a succeeded deployment, which only a live deployment can produce.
- `teardown` handles containers one at a time and stops at the first one that does not come out clean (failed removal or refusal); the rest are reported "not attempted" and get no journal record. (The shared engine's `execute_actions` would otherwise carry on with later actions; teardown calls it per action rather than changing the engine.)

## B7 live acceptance
The CLI's first live use (one `deploy` with the separately built experimental binary, `teardown` with the shipping binary, read-only `status`/`doctor`/`plan`/`inventory` throughout) is recorded in [`DEPLOY_B7_LIVE_RESULTS.md`](DEPLOY_B7_LIVE_RESULTS.md). The shipping gate remains closed.


## B8.1 — recovery behaviour, known limits and test fidelity
Offline only: no code in the CLI, engine, backend or journal was changed, the shipping gate is still closed, and nothing here was run against a Docker Engine. Background: the B8 failure and recovery audit.

### Ownership refusal is terminal (F1)
- When a cleanup is **refused** because ownership cannot be proven (the recorded id, name or any of the three `io.harpoon.*` labels no longer match), the ledger entry becomes `Refused`. `reconcile` and `teardown` never re-evaluate it, even if the labels later reappear, and `status` keeps listing it as "not touched, ownership not proven".
- While any entry is unresolved, `deploy` is refused for the **whole runtime directory** (exit 6). A refusal therefore blocks every further deployment from that directory.
- **No acknowledgement, forget, force or override command exists and none is authorized.** The object is never touched. The only way forward today is to investigate it by hand in Docker and use a new runtime directory.
- This is deliberate fail-closed behaviour, pinned by `a_refused_ownership_stays_terminal_and_blocks_the_runtime_directory`. Whether a future, explicitly authorized acknowledgement should exist is an open design question and is not part of B8.1.

### Retained images of failed deployments (F2)
- The image a deployment builds is `Retain`. After a **failed or interrupted** deployment it is kept, still labelled, and is **not** reported as unresolved cleanup; `deploy` for another application is not blocked by it.
- There is **no automated removal path** for it: `reconcile` skips retained entries, and `teardown` accepts only a `succeeded` deployment. Until a cleanup operation exists, it stays in Docker (disk use grows with each failed deployment) and can only be removed by hand.
- **Proposed, not implemented in B8.1:** an explicitly authorized, ownership-verified operation that removes the retained image of one named failed or interrupted deployment — by recorded image id, only if all three labels still match and no container uses it, with a dry run by default, an exact authorization phrase, a journal attempt/result record, no force, no prune and no name patterns. It needs its own review before any code.

> **Update (B8.2):** the missing removal path described above now exists as `cleanup-image`; see "B8.2 — `cleanup-image`" below. The F2 text above is kept as the B8.1 record of the gap.

### What the new tests exercise
| Kind | Tests | What it is |
|---|---|---|
| Real `DockerBackend` + real engine against a **simulated** Docker server and a fake Buildx | `deploy-docker/tests/backend.rs`: `a_crash_at_every_write_is_recovered_by_label_with_the_real_backend` (healthy), `…_of_a_failed_preliminary_check_…`, `…_of_a_failed_final_check_…`, `…_of_a_port_collision_…`, `…_of_a_failed_removal_…` | A crash at every journal write of each scenario, then restart: the deployment is marked interrupted, a new backend reconciles by label, and the tests assert that no unrelated container or image (nor the base image) is touched, no request is forced or a prune, a ledger entry recorded as released really is gone, no entry is left "intended", and replaying every record written before and after the restart reproduces the final accounting. The failed-removal case keeps Docker refusing removals after the restart (nothing may be recorded as released that still exists), then lets Docker recover. |
| Real **CLI**, real journal files, real `DockerBackend`, **simulated** Docker server | `deploy-cli/tests/cli.rs`: `an_interrupted_deployment_is_recovered_reconciled_and_only_then_lets_the_next_one_run`, `a_reconcile_that_cannot_finish_…`, `a_refused_ownership_stays_terminal_…` | A gate-open `deploy` runs in a separate process and is killed with SIGKILL while it starts its preliminary container. Then: `deploy` refused → `recover` (Docker not contacted) → still refused → `reconcile` dry run (writes nothing) → `reconcile --apply` (only the recorded id, `force=false`, no image deleted) → `status` clean → the next deployment is permitted. The second and third tests show a reconcile that cannot finish, and a refused ownership, keep the runtime blocked. |
| Against a **real Docker Engine** | none of the above | The only real-engine evidence remains the B7 live acceptance. Failure and interrupted-recovery paths have not been run against a real engine. |

Remaining gaps after B8.1 (unchanged by it): live failure and interrupted-recovery behaviour against a real engine; the BuildKit daemon's behaviour after a build is killed at its deadline; a container that ignores the stop signal; guest private-NAT exposure; intermittent forwarder startup lag; release replacement and rollback (engine state machine only, no Docker implementation); F1 and F2 above.

## B8.2 — `cleanup-image`
Offline only: implemented and tested against a *simulated* Docker server. It has **not** been run against a real Docker Engine, the shipping deploy gate is unchanged, and `teardown` for successful deployments is unchanged.

```
harpoon-deploy cleanup-image --deployment dep_… --image sha256:<64 hex> --docker-socket PATH --resources-dir DIR
    [--runtime-dir DIR] [--apply --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE]
```
Dry run by default; `--apply` needs the same exact authorization phrase as `reconcile --apply` and `teardown --apply`, and takes the journal's single-writer lock. `--image` is a typed confirmation: a full id that must equal the id recorded for that deployment (a tag, a prefix or upper case is a usage error). There is no force, prune or pattern option. Exit codes: 0 done or nothing to do; 2 usage; 4 journal (corrupt, locked); 5 Docker unreachable or ambiguous answer (nothing written); 6 refused (nothing written, except where stated below); 7 attempted and incomplete (the state is recorded).

### Eligibility (all must hold; otherwise it refuses and, except for an ownership mismatch, writes nothing)
Journal, checked before Docker is read:
1. The runtime is a Docker (not simulated) runtime with a journal; the deployment is in it.
2. The deployment finished without succeeding (`failed`, `cancelled` or `interrupted`). A running or queued one is told to `recover`; a succeeded one is told to use `teardown`.
3. It records exactly this image id, with disposition `Retain` and a receipt. An image with no receipt is resolved by `reconcile` (which adopts it), not here.
4. The entry is not already released (then: "nothing to do") and was not refused earlier (terminal, F1 — see below).
5. Every other resource of that deployment is released (containers and leases: run `reconcile` first).
6. No other deployment has an unreleased entry for the same image id.

Docker, read-only, and checked again immediately before the deletion:
7. `GET /images/{id}/json` returns that exact id, and it carries all three `io.harpoon.*` labels with this deployment's values.
8. Its tags are a subset of the recorded tag (none at all is fine), and it has no registry digest.
9. `GET /containers/json?all=true` shows no container, running or stopped, created from it, and no container whose image Docker does not report.

Then `DELETE /images/<id>?force=false&noprune=true`, by id only. Docker has the last word: it re-checks container references, other tags and child images at that moment and answers HTTP 409 if any exist.

### What is recorded, and what blocks
- The existing journal records are used; **the journal format is unchanged**. `CleanupAttempt` (the durable intent) is written before Docker is asked, `CleanupResult` after. A crash in between leaves an outstanding attempt; a repeat re-checks everything and either deletes or finds the image gone (recorded as already absent). A deletion is never recorded as released unless Docker removed it or reports it gone.
- **Ownership mismatch** (the id Docker returns differs, or a label is missing or different) is the terminal F1 refusal, recorded exactly as `reconcile` and `teardown` record it. It stays blocking, a repeat of the command or restored labels do not reopen it, and nothing deletes the image. F1 is unchanged.
- **Not exclusive** problems found by the command's own checks (another tag, a digest, a container, a shared ledger entry, other resources of the deployment not released) are refused **before any journal write**: nothing is recorded, nothing blocks, and a repeat works once the cause is gone.
- **A conflict at deletion time** — Docker answers HTTP status 409, or the command's own pre-delete re-check finds a new cause after the attempt was journaled — is recorded as a **typed journal outcome**, `conflict` (cause `docker-refused` or `not-exclusive`), which replays as the ledger state `CleanupConflict`. It is not a kind of failure message: only the backend's structured result (the status code, or Harpoon's own check) can produce it, a `Failed` message with any text can never become one, and the ledger rejects a conflict outcome anywhere but a retained image that has a receipt. The image is not released and the command exits 7. **This does not block other deployments** only under the narrowest rule (`Ledger::deferred_image_conflict`): a `Retain` image with a receipt, in state `CleanupConflict`, of a deployment known to have finished without succeeding, whose every other entry is released, with no later attempt outstanding. It is re-derived from the replayed journal on every command, so it survives a restart. `status` then lists it as a "retained image" with the reason, and a repeat of `cleanup-image` retries it (a second attempt, never a force).
- **Everything else stays blocking**: a timeout, a disconnect, a 5xx, a malformed answer, a reply lost after the delete, an attempt with no recorded result, any ownership mismatch, an unreceipted intent, an active container or lease of the deployment, a succeeded deployment. These are recorded as an ordinary failed cleanup (never as a conflict, whatever the text says) and block `deploy` in that runtime directory until a repeat settles them. A conflict that does not meet every condition above (for example, the deployment still has an active container) is also still blocking.
- `status` and `recover` list retained images of unsuccessful deployments ("they do not block deployments; remove one with `cleanup-image`").

### Fake versus real Docker
The test Docker server's image deletion now follows Docker's *documented* behaviour instead of always succeeding: without `force`, an image used by a running or stopped container, with dependent child images, or addressed by id while several tags point at it, is a 409 and nothing is removed; addressed by one of several tags it is only untagged. **That behaviour has not been observed on a real Engine in this repository** (the only real deletion seen is the B7 teardown's success path). Whether the real daemon answers 409 in each of these cases, and what it does for images in the containerd image store, is unverified until a separately authorized live validation.

### Journal format, versioning and compatibility
- The typed conflict is a new `result` outcome token (`conflict-docker-refused`, `conflict-not-exclusive`) in the existing checksummed line format. **No existing record changes**, so the file header stays version 1, journals written before this change replay exactly as before, and nothing is migrated or rewritten.
- A build that does not know the token **refuses the journal**: its decoder answers `bad outcome` for any token it does not know, and a damaged or undecodable terminated line is corruption, so every command reports the journal unusable (exit 4) and changes nothing. This was verified against the pre-change reader (`journal line 5 is damaged: bad outcome`). The consequence is that, once a conflict has been recorded, an older Harpoon build can no longer open that runtime directory; that is loud and fail-closed, never a silent reinterpretation. Downgrading after a conflict has been recorded therefore needs a new runtime directory (or the journal exported for investigation with `export-journal`).
- Unknown tokens, the wrong number of fields, a damaged checksum and a conflict outcome in a place the ledger does not allow (a container, a lease, an image that is not `Retain`, an image with no receipt, a result with no attempt) all make the journal unusable (`Corrupt` or `Inconsistent`). Tests cover each.
- No cryptographic authentication of journal records is added; whether one is needed is a separate threat-model decision. The journal is trusted to the same degree as before (mode 0600, checksummed, single writer).
- Image tag and digest lists from Docker are parsed strictly: `null` means none, but a missing key, a non-array or a non-string element is a malformed answer (an error, nothing deleted, not "no other tags").

### Remaining gaps after B8.2
No live validation of any of this; real 409 behaviour (including child images and the containerd image store) unobserved; `teardown` of a successful deployment does not yet apply the exclusivity checks (tags, digests) that `cleanup-image` applies; a retained image cannot be removed while another tag points at it (by design — remove that tag yourself); the F1 terminal refusal still has no acknowledgement or override.


## B13 — runtime variables, failure output, forwarder default

- `--env KEY=VALUE` (repeatable) on `plan`, `deploy` and `replace`. **Non-secret only**: values are visible in `docker inspect`
  and stored in the journal; Harpoon prints names only. `HARPOON_*` is reserved (the per-container
  `HARPOON_DEPLOYMENT_MARKER` is generated, never configured). Each release is pinned with its own variables; a restoration or
  rollback reuses that release's pin. See `DEPLOY_B13_ACCEPTANCE.md` for validation limits and the pin format (`v1` unchanged,
  `v2` only when variables exist).
- A candidate that fails before any cutover prints what stayed untouched and the real next commands (`status`, the deliberate
  re-run, `cleanup-image`).
- `--forwarder-command` defaults to `harpoon`, the live runtime's real process name; the check remains exact.
- `status` words an unfinished or restored cutover accurately (see the B13 document).

## B14 — pauses, held events, resume

- `resume --app A (--dismiss | --newest | --revision REV)` records one explicit decision; it never contacts Docker and a
  selection deploys nothing (pending attended execution; the application stays blocked until dismissed). `events [--app A]`
  lists the pause and event history. `status` shows pauses (with since-time and origin), held events and decisions. There is
  no command that creates events: the engine's `control::ingest` is the interface a future trigger integration uses.
