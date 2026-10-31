# Bounded cutover recovery — offline slice

Status: implemented and tested **offline only**. Live replacement is not enabled; the shipping gate
(`Gate::shipping()`) is unchanged and closed. No Docker or VM was contacted. Not production readiness.
Explicit rollback of a completed deployment is the next milestone and is not part of this slice.

## What exists

| Requirement | Implementation | Evidence |
|---|---|---|
| Pin the immutable, non-secret configuration of a release | `ReleaseConfig` (`deploy/src/release.rs`): exact image id (`sha256:…`, a tag is rejected), tag for display, container and host port, health spec, memory/CPU/PIDs limits. Stored in the deployment snapshot (`Deployment.pinned`), set before the final check, appended to the snapshot only when present so older snapshots decode unchanged. No environment values, no secrets; the per-container response marker is regenerated for every container | `deploy/tests/b11_recovery.rs`, `deploy-journal/tests/sqlite.rs`, unit test in `release.rs` |
| Restore after a failed confirmed cutover | `Run::restore_phase` calls `Backend::restore_previous(io, previous, budget)` once. The Docker backend re-checks the pinned image id and the previous release's labels, serves on the previous release's still-held lease, creates a new container (ledger intent → create → receipt, disposition `Retain`) from the pinned image id with the pinned limits and port, starts it and runs the **pinned** health check | `deploy-docker/tests/b11_recovery.rs` (fake Docker) |
| One bounded attempt, default 60 s | `Policy.restore_budget_ms` (60 000); the backend limits every call to the remaining budget; the engine treats a result that arrives past the budget as unverified (`restored_unverified`) and a backend timeout as `restore_failed(timeout)`. No retry exists | tests: budget passed through, late answers, 1 ms budget against a slow Docker, one `start` request only |
| Exact outcome, persisted and reported | `restored`, `restored_unverified`, `restore_failed(reason)`, `not_attempted(reason)` with reasons `config_unavailable`, `image_missing`, `docker_unreachable`, `timeout`, `start_failed`, `interrupted`. Recorded as `InProgress` **before** the backend is called, then final; printed by `deploy`/`replace`/`restore` and `status` | engine, Docker and CLI tests |
| Measured interruption and restoration duration | `Deployment.interruption` (stop → restored/confirmed) and `Recovery.started_ms/finished_ms`; shown in `status` and by the commands. Simulated runs say "simulated clock" | engine (1 500 ms and 500 ms from scripted timings), CLI |
| Pause triggers only after a real failed cutover | `trigger_pauses(deployments)` is derived from the durable records: a cutover that failed, or began and never finished. Build, preliminary-check and unconfirmed-plan outcomes never pause. Shown in `status`. There is no automatic trigger yet, and no `resume` | engine + CLI tests |
| Preserve the failed candidate; touch nothing else | new `EndState::FailedCutover`: the candidate's `RemoveOnFailure` container and lease are kept (stopped) with its retained image and logs; temporary preliminary resources are still removed. The previous release's container only ever receives `stop` (verified by id, name and all three labels); unrelated applications, unlabelled containers, volumes, networks are never named in a request | Docker tests inspect every request |
| Crash during cutover or restoration | `recover_on_start` marks the deployment `Interrupted`, never resumes, closes an unfinished restoration as `restore_failed(interrupted)`, and `application_may_be_down()` stays true. `restore --deployment ID [--apply]` (simulate only) makes the explicit attempt: allowed once, only for an interrupted deployment whose cutover began and which has no recorded attempt (even an unfinished one forbids another). The store accepts exactly this change to an `Interrupted` deployment (none → in progress → one final outcome) and rejects any other, failing closed | crash sweeps at every write (engine and Docker), `replay` tests, SQLite restart test, CLI tests |
| SQLite durability, record-before-side-effect, ownership, fail closed | snapshot of the attempt is written before restoration starts; all created objects go through the ledger; removal and stop re-verify labels; damaged or inconsistent records make every command exit 4 | existing suites plus the above |

## Boundaries kept

* `DockerConfig.enable_replacement` is `false` by default, is never set by the CLI, and `DockerBackend::supports_cutover()` returns it. Against a real Engine the engine therefore still stops after the preliminary check.
* `replace` and `restore` refuse without `--simulate`, before anything is read.
* No automatic rollback of a completed deployment; no zero-downtime proxy, secrets, Git, UI or daemon.

## Remaining limitations

* **Live behaviour is unproven.** Every Docker test uses a simulated server. Real container stop/start semantics, port release timing and the forwarder's sync lag (up to 10 s) are untested, and the measured interruption has no live value.
* **Pin timing.** The pin is saved after the new container starts and before its final check; a crash in that gap leaves a serving release without a pin, which later reports `not_attempted(config_unavailable)`. A release deployed before this change has no pin either.
* **A restored container belongs to the failed deployment's ledger** (labelled `io.harpoon.restores=<previous deployment>`) while the stopped original stays recorded under the previous deployment. After a restoration the next replacement, `teardown` and retention logic do not yet know to treat the restored container as the serving release.
* **The failed candidate can still hold the port** when the process died mid-cutover; the explicit `restore` makes one attempt only and does not remove it, so `reconcile` should be run first. Candidate containers are kept until `reconcile`/`teardown` is run by the developer.
* **Pause has no consumer and no `resume`.** It is derived and shown; clearing it is a later milestone.
* **The prepared-candidate handoff is not solved**: a confirmed `replace` still rebuilds (see B10).
* A restoration cannot restore data, volumes or schema; it recreates the container only.

## Evidence needed before live replacement is enabled

1. A live run on a disposable application: replacement succeeds, with the measured interruption and the real port-forwarder lag recorded.
2. A live forced cutover failure (candidate fails its final check): restoration returns `restored` with its own health check, the previous image/configuration are the pinned ones, and the failed candidate is kept.
3. Live `restore_failed` / `not_attempted` cases (image removed, port taken) behave as recorded, without touching any other resource; exact inventory restored afterwards (the B8.3 standard).
4. A live kill of `deploy` during the cutover and during the restoration, followed by `recover`, `status` and the explicit `restore` exactly once.
5. Resolution of the restored-container ownership/retention limitation above, so the next replacement, `teardown` and reconcile treat the restored container as the serving release.
6. A decision on pinning before the new container starts (closing the pin-timing gap), and explicit written authorization for each live step, with the shipping gate still opened only in a separately built, separately authorized binary.
