# B12 — Explicit manual rollback (offline slice)

Status: **offline only; live replacement and live rollback are disabled; the shipping gate is closed.**
Nothing here was run against Docker or the Harpoon VM, and nothing here claims production readiness.
Requirements: D4, D5, D10. Builds on B10 (replacement), B11 (bounded recovery, hardening #127).

## What a rollback is

A developer-initiated replacement whose candidate is an **earlier release**, re-served from its
retained image and pinned configuration. It reuses the existing cutover, ownership, health-check and
bounded-recovery mechanisms; there is no second recovery engine. It is never automatic.

## What exists

| Requirement | Implementation | Evidence |
|---|---|---|
| Release selection | `releases --app` lists succeeded releases newest first, marks the serving one, and gives every reason a release is not a target. `rollback --to REL` takes the **exact release id**; a name, tag, "previous" or another application's release is refused. | `deploy/tests/rollback.rs` (selection, unprovable targets, other applications); `deploy-cli/tests/rollback` |
| Provable target | Eligible only with a pinned configuration naming a `sha256:` image id, a retained, unreleased image of that deployment in the ledger, and a release earlier than the serving one (moving forward is a replacement). | same |
| Pinned configuration | The Docker backend in rollback mode builds nothing: its source step inspects the pinned image **id**, requires the id and the target release's own ownership labels, and fails before anything changes if the image is gone or foreign. Containers use the pinned image id, container port, health check and limits; only the host port is chosen afresh through the ledger. | `deploy-docker/tests/rollback.rs` |
| Confirmation | The plan (target, serving release, interruption bound, one bounded recovery attempt) and "Application rollback only. Data, volumes and database schema are NOT rolled back." print before anything is stopped. Without `--confirm-rollback` the target is prepared and checked and the serving release is untouched. | `the_plan_carries_the_data_warning…`; CLI `without_confirmation…` |
| Safe rollback | The serving release runs until the target passes its preliminary check. Failure at the source step, plan, or preliminary check never stops it. Failure after the cutover began uses the existing single bounded restoration of the release that was serving. Success needs the final health check. | engine, Docker and CLI tests |
| Durable state | `Deployment.rollback_to` (target) is written with the first save, so a crash at any point still says what was being rolled back to; `replaces` (source), the pin, step timestamps, interruption, failure and recovery are the existing records. Snapshot field 3; snapshots of other deployments encode exactly as before; a terminal record's `rollback_to` cannot be rewritten (the store fails closed). | `a_rollback_record_survives_restarts…` (SQLite), round-trip test |
| Serving release | `serving_release` reports the rollback's own container as serving the **target release** (`ServedBy::RolledBack`) with the configuration it actually serves with (`Serving.config`), after a failed rollback it reports the restored container of the release that was serving, and an unconfirmed or early-failed rollback changes nothing. `status`, `replace`, `teardown`/`reconcile` targets use it (a later replacement restores what really serves, including its fresh port). | engine, Docker, CLI tests |
| Trigger pause | A successful rollback is a pause reason derived from the record (`TriggerPause.rollback`), shown by `status` with the target release; there is no automatic resume. A rollback that stopped the release and failed shows the existing failed-cutover pause. | engine and CLI tests |
| Crashes | A write-by-write crash sweep of a failing rollback: the record always names its target; recovery never resumes, never starts a restoration or a second rollback, never leaves an attempt "in progress", and replays. An interrupted rollback is "application may be down", blocks any new rollback or replacement, and is offered the existing explicit one-attempt `restore`. | `a_crash_at_every_write…`, `an_interrupted_rollback…` |
| Rollback after a restoration | The serving container may be a restored one recorded under a failed deployment; the rollback stops that container and the new serving release is derived correctly. | engine and CLI tests |
| Unrelated resources | The Docker tests assert no prune, volume, network, exec, commit, build or force request; no unrelated container touched; no image created, changed or removed; the other application's journal lines identical before and after. | `nothing_unrelated_or_destructive` |

Retained releases are never silently removed: a rollback records no image of its own and removes
nothing; the superseded release's container is only stopped, and the failed candidate of a failed
rollback is kept for inspection.

## What stays disabled

`rollback` refuses without `--simulate` before reading anything. `DockerConfig.enable_replacement`
defaults to false and the CLI never sets it. `DockerBackend::with_rollback` is reachable only from
library code and tests.

## Remaining limitations

- **Live behaviour is unmeasured.** Real stop/start timing, forwarder lag after a port change, and the
  actual interruption of a rollback have not been observed. Interruption figures are bounds from the
  configured budgets, or simulated.
- **Application only.** Data, volumes and database schema are not rolled back; images that declare
  volumes are refused by the existing check. Nothing detects schema incompatibility between releases;
  the developer is warned, not protected.
- **Retention is not guaranteed.** A retained image is "retained" because nothing removes it, but
  `cleanup-image` can; the ledger then stops listing the release as a target. Rollback targets are not
  protected from that command.
- **Targets are releases that succeeded with a pin.** Releases from before pinning (and the first
  release if it predates it) are listed with the reason and cannot be rolled back to.
- **Forward moves are replacements.** After a rollback, the release that was rolled back from is not an
  eligible target (`not earlier`); returning to it is a new deployment.
- **No resume.** The trigger pause after a rollback has no consumer and no `resume` command yet; no
  automatic trigger exists, so nothing is paused in practice.
- **Teardown by a rollback's own id** is not offered; dismantling the target release's deployment id
  includes the rollback container and port lease, and `reconcile` handles the rest.
- **Simulated CLI.** `rollback --simulate` drives the fake backend; the Docker rollback path is covered
  by fake-Docker library tests, not by a CLI run.
- The pin-timing window after a collision-moved port and single-listing blocker detection (B11
  limitations) apply unchanged.

## Evidence needed before enabling live rollback

Everything B11 lists for live replacement, plus on a disposable application: a real rollback to a
retained image with measured interruption; a rollback whose final check fails and is restored; a
rollback after a restored deployment; image removal by `cleanup-image` followed by a refused
rollback; and confirmation that no unrelated container, image, volume or network changed. Until
then the CLI refuses and the gate stays closed.
