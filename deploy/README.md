# harpoon-deploy — offline foundation

Milestone B, step 0 of the Harpoon 2.1 deployment control plane (see
`docs/deployment-control-plane-requirements.md`, D4/D5/D10). Pure Rust, **no dependencies**,
no I/O.

## In scope
- `ids` — stable, typed, sortable identifiers (`prj_…`, `app_…`, `dep_…`, `rel_…`) and the three
  Docker label keys. A release shares its characters with the deployment that built it.
- `state` — deployment statuses and the complete transition relation, steps, the cutover
  journal, recovery outcomes (`restored`, `restored_unverified`, `restore_failed`,
  `not_attempted`), interruption timing, and `recover_on_start` (marks non-terminal
  deployments `interrupted`, flags incomplete cutovers, idempotent).
- `queue` — per-application serialization; newest queued request supersedes an older queued
  one; the running one is never interrupted.
- `backend` — the *local* execution seam (`Backend`) and a scripted `FakeBackend`.
- `engine` — `run_deployment`, the lifecycle driver, and a `Journal` trait with an in-memory
  implementation that can "crash" at any write.

## Explicitly out of scope
Docker or VM calls, network listeners, background processes, SQLite, Keychain, Git, a
serialization format, the UI data contract, a provider abstraction, automatic triggers,
paused-trigger/held-event state, rollback, promote, and any decision still open in
`docs/results/COMPAT_BUILDX_AND_PORT_FORWARDING.md` (publish form, buildx, probe mechanism).
Status and step strings (`as_str`) are provisional until the persistence schema exists.

## Guarantees under test (`cargo test --offline`)
- Nothing is stopped before the build and preliminary check pass; an ordinary failure never
  stops anything, never attempts a restoration, and never pauses triggers.
- The cutover journal is written before the previous release is stopped, and every side
  effect is preceded by a write describing it (checked through a shared event trace).
- Only a failure after the cutover began triggers **one** bounded restoration; its outcome is
  recorded; a failed restoration leaves the interruption open and the app "possibly down".
- A crash at **every** write of five scenarios recovers to a consistent state, with the
  incomplete-cutover flag matching the journal.
- Cancellation is honored only before the cutover begins.
- The transition table is checked exhaustively (8 × 8) against the documented relation.

Mutation checks run during development (reordering the journal write, stopping early,
restoring twice, pausing on any failure) were each caught by at least one test.

## Run
```sh
cd deploy && cargo test --offline
```
