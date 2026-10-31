# Health-gated replacement — offline slice

Status: implemented and tested **offline only**. Live replacement is not enabled. The shipping gate
(`Gate::shipping()`) is unchanged and closed. No Docker or VM was contacted. Not production readiness.

## What exists

| Requirement | Where | Evidence |
|---|---|---|
| Build and prepare a candidate without stopping the current release | engine order: source → build → preliminary check all precede the cutover; the Docker backend runs the preliminary container on a test-range port that the current release does not use | `deploy-docker/tests/b9_replacement.rs` (current container receives only GETs; stays running) |
| Preliminary health evidence, durable | a passing check's summary is now stored in the step record (`health evidence[ (simulated)]: …`) in SQLite | `deploy/tests/b9_replacement.rs`, `deploy-docker/tests/b9_replacement.rs` |
| Failure before cutover keeps the current release; failure evidence retained | build or preliminary failure → `Failed`, cutover `NotStarted`, no pause, no confirmation asked; candidate image kept (`cleanup-image`), failure message, evidence and the failing step's log tail are shown (`print_report`) | same tests, CLI output |
| Cutover plan and anticipated interruption before confirmation | `CutoverPlan` (engine): previous/candidate release, upper bound `2 × cutover budget + final-check budget`, wording says "unmeasured" and makes no zero-downtime claim; `replace` prints it before the build result | `deploy-cli/tests/b9/mod.rs` |
| Confirmation | `run_deployment_confirmed(..., confirm)` is asked only after the preliminary check passed and before `begin_cutover`; a refusal ends the deployment `Cancelled`, reports `Prepared { NotConfirmed }`, never pauses triggers | engine tests |
| Measured interruption recorded | `Deployment.interruption` (started at `begin_cutover`, ended at `confirm_cutover`) is persisted in the SQLite snapshot; `status` shows it; simulated runs say "simulated clock" | CLI test: 1000 ms from the fake's scripted 500 + 300 + 200 |
| Release identity, labels, durable transitions, crash recovery | unchanged code paths: `ReleaseId::from_deployment`, three ownership labels, snapshot-before-side-effect ordering; an unconfirmed cutover (`Begun`) is still detected and blocks replacement | existing 150+ engine/backend tests plus `an_unfinished_cutover_blocks_replacement…` |

## The command

`harpoon-deploy replace --simulate --app APP --runtime-dir DIR --project-dir … [--confirm-cutover]`

* Without `--simulate` it is refused (exit 6) before anything is read, written or contacted, regardless of gate or authorization.
* Without `--confirm-cutover` it prepares the candidate, prints the plan, and stops with the current release untouched (exit 0). The candidate image stays.
* With `--confirm-cutover` it performs the simulated cutover and records the interruption.
* The current release is the newest succeeded deployment of the application whose serving container is still recorded and unreleased. No running release, unresolved ownership, an unfinished deployment, or a possibly-down application (cutover begun, never confirmed) refuse the command and write nothing.

## What the Docker backend does now

`DockerBackend::supports_cutover()` is `false`. Against a running release the engine therefore stops after the preliminary check, before `begin_cutover`, and reports the candidate as prepared. This also fixes a latent hazard: previously an engine run with a previous release would have begun a cutover, failed in `stop_current`, and flagged a "failed cutover" (pausing triggers) though nothing was stopped.

## Remaining limitations

* **No live replacement.** Bounded cutover recovery (restore from the retained image and pinned configuration within a budget, with a recorded outcome) is the next milestone; `restore_previous` still reports `NotAttempted`.
* **Pinned non-secret configuration is not stored.** Nothing yet snapshots the container port, health path or limits of a release, so a restore could not be reproduced; this belongs to the recovery milestone.
* **Two-step confirmation re-runs the build.** The CLI has no prompt or prepared-candidate handoff; the confirmed run builds again. A reusable prepared candidate is not implemented.
* **Interruption is an upper bound plus a simulated measurement.** The real interruption (stop, start, published-port forwarder lag of up to 10 s, final check) has not been measured on a live engine.
* **Superseded releases are not cleaned up.** After a replacement the older release's records remain; retention (keep current plus N) and rollback are later milestones.
* **Simulated runs cannot script failures from the CLI**; build and preliminary failures are covered at the engine and Docker-backend (fake Docker) layers.
* The Docker behavior tested here is a simulated Docker server, not a real Engine.
