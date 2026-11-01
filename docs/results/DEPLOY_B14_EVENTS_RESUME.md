# B14 — Durable pauses, held events and explicit resume

Status: offline implementation. **Nothing was run live; the shipping gate is closed and unchanged; Milestone B is not
accepted.** There is no event source and no source executor in Milestone B, so this adds the durable *state* and the
explicit *decision* mechanism, not event delivery and not deployment of a held revision.

## 1. What exists

Three append-only record kinds share the deployment journal (SQLite). Replaying them in order rebuilds the exact state;
nothing depends on the clock or on anything outside the journal.

| Record | Meaning | Written |
|---|---|---|
| `pause` | application, originating deployment, kind (`failed_cutover`, `interrupted_cutover`, `rollback`), time, context | once per pause-causing deployment, by the first writer that sees it (`deploy`, `replace`, `rollback`, `restore`, `recover`, `resume`, event ingress) |
| `event` | application, delivery id, source, revision, received time | by `ingest`, only while the application is paused |
| `resume` | one developer decision, in one record: choice, selected delivery, dismissed and superseded deliveries (with why), pauses cleared | by `resume` |

- **What pauses** (unchanged): a cutover that failed, a cutover that began and never ended (interrupted), a successful
  manual rollback. **What does not:** a build or source failure, a failed preliminary check, a candidate prepared but
  never confirmed (no cutover began), a cancelled run. The pause time is a fact from the deployment record (when the
  cutover began or failed; when the rollback finished), so a pause recorded late carries the same time.
- **The pause in force** is every pause-causing deployment that no `resume` record has cleared. A cleared pause is not
  re-derived from the older deployment; a **new** failed cutover is a new deployment and pauses again. If a pause record
  could not be written yet, the pause still holds (derived) and `status` says so.
- **Event states:** `held`, `selected`, `dismissed`, `superseded` (with `newer_selected` or `not_chosen` and the delivery
  that superseded it). Nothing is ever deleted; the history of every event stays.
- **Ingress** is one function, `harpoon_deploy::control::ingest`. A paused application records the event as `held`; an
  unpaused one records nothing, not even a pause of another application, and returns `NotPaused` (the caller decides); the same delivery id with the same content is a
  no-op (`Duplicate`); the same id with different content is refused; tokens are validated (delivery id, source and revision:
  letters, digits and `. _ -`, plus `:` or `/` where noted, at most 128 bytes) before anything is written. Applications
  are isolated (the key is application + delivery id). **No command, fixture generator or server feeds it**; the tests call it
  as a future Git/CI integration would.
- **Replay fails closed.** A decision naming an unknown or non-held delivery, a dismissal that leaves a held event
  unaccounted for, a second selection while one is pending, a duplicate delivery, a pause recorded twice, a decision clearing a
  deployment that never paused its application, or a pause that does not match a pause-causing deployment of its
  own application: the store refuses to open. A pause must name a deployment whose record causes a pause (failed or
  interrupted cutover, successful rollback) with the same kind and time; the one legitimate change is an interrupted
  cutover recorded while open and failed since, whose pause time must equal the cutover's original start
  (`interruption.started_ms`, persisted by `begin_cutover`); with no recorded start, replay fails closed.

## 2. Commands

- `harpoon-deploy resume --app A` with **exactly one** of:
  - `--dismiss`: dismisses every held event and a pending selection (all stay in the history) and clears the application's
    pauses. A later failed cutover or rollback pauses it again.
  - `--newest`: selects the newest held event (latest received, then latest recorded); older held events are recorded
    `superseded (newer_selected)`.
  - `--revision REV`: selects the newest held event with exactly that revision; an unknown revision is refused with the
    held revisions listed; the other held events are recorded `superseded (not_chosen)`.

  It never contacts Docker (it accepts no Docker flag), never builds, starts or stops anything, and creates no journal.
  A refusal (nothing to select, an unknown revision, a selection already pending, a writer holding the journal) writes
  nothing, not even a pause record for a legacy deployment whose pause is only derived: every choice is validated
  before any write, and only an accepted dismissal records that application's pauses and then its one decision.
  Repeating a decision is idempotent: the same revision again, or a second dismissal, writes nothing.
- `harpoon-deploy events [--app A]`: read-only history of pause records and every event with its status and decision.
- `status` shows, per application: paused or not, why, since when (epoch ms), the originating deployment, whether the
  pause is recorded or only derived, the held count with each held revision, delivery id, source and time, the history of
  settled events, any pending selection, and the available actions.

## 3. What a resume does — and what it does not

- **Dismiss** is complete: the pause is cleared and the events are dismissed. Nothing is deployed because nothing was chosen.
- **Select** records the developer's choice only. **Nothing is deployed**, and `status` says so in capitals. There is no Git
  source executor in Milestone B, so the choice is *pending attended execution*. Clearing the pause on a selection would
  let an automatic trigger promote a different revision before the chosen one ran, so **the pause stays and the application
  stays blocked** (new events are still held) until the selection is dismissed. Choosing a second revision while one is
  pending is refused.
- A recorded decision is never described as serving: the `serving:` identity is still derived only from deployment and
  container records, and the status distinguishes deployment history, Docker state (not consulted here) and decisions.
- **Deferred until a source executor exists (Milestone D / Git):** consuming a pending selection (checking out the revision,
  deploying it, recording that it executed and then clearing the pause), real event delivery, queue-level `superseded`
  records for queued deployment requests, and pause time limits.

## 4. SQLite compatibility

- New record kinds only: no schema migration (`schema_migrations` is untouched). A store that never held a pause or event
  has none of them and reads exactly as before (test: a store with only a deployment replays with an empty control state;
  all earlier legacy-journal tests still pass).
- A pause record is written by the first writer after a pause-causing deployment. From then on a **pre-B14 build cannot open
  that store**: it fails closed on the unknown record kind rather than ignoring a pause. A store no B14 writer has touched
  after such a deployment stays readable by older builds.
- Damage inside a pause, event or resume record fails its checksum and the store refuses to open (`status`, `events`,
  `recover` and `resume` exit 4 and leave the file untouched). A well-formed record that contradicts the state is refused
  by replay (see 1).
- A second writer is refused at once (`another Harpoon process holds the journal`) and the history is byte-identical
  (four threads tried).

## 5. Crash testing

- **Every write, engine level** (`MemoryJournal::crash_at`): the full sequence (two pauses, four events including a
  duplicate delivery, a newest-wins selection, a dismissal) is 8 writes. For each of the 8 crash points the journal keeps a
  valid prefix; it replays; the same requests made again (conflicts tolerated, as a retried tool would) end in a state
  **equal** to the uninterrupted run, and one more restart reconstructs it. 8 of 8 crashes recovered.
- **Every prefix, SQLite level:** the 6 control records of a sequence are written to a real SQLite store as every prefix
  0..=6 (a store cut after each write), reopened (replay verified), completed again and compared with the uninterrupted
  state, from disk as well. 7 of 7.
- A decision is one record, so a crash cannot half-apply it; a pause and its events are separate records, but an event is
  only written after its pause record.

## 6. Tests

| Crate | Passing | New for B14 |
|---|---|---|
| deploy | 213 | 12 (pause rules, idempotency, isolation, malformed input, newest-wins, explicit revision, dismissal and re-pause, codec and damage, replay refusals, crash at every write) |
| deploy-journal | 39 | 4 (restart replay and legacy stores, damaged/inconsistent state fails closed, concurrent writers, cut after every write) |
| deploy-docker | 220 | 0 (unchanged) |
| deploy-cli | 146 | 11 (durable pause from a failed cutover, rollback pause, unconfirmed/preliminary/successful runs record none, held/duplicate/isolated events, newest selection stays blocked, explicit revision and invalid revision, one explicit choice, pause again after resume, no Docker, writer/corruption, legacy-shaped store) |

fmt, Clippy `-D warnings` and the source-guard greps of all four crates are clean.

## 7. Milestone B acceptance — what changes

| Item | Before B14 | Now |
|---|---|---|
| Durable pause with reason, time, origin | derived only (no time) | **Offline**: recorded, replayable, cleared only by an explicit decision |
| Events held while paused, none lost, idempotent | **None** | **Offline** (engine ingress and fixtures) — real event delivery **unavailable until Milestone D** |
| Explicit resume (dismiss / newest / specific) | **None** | **Offline**: decision recorded; **actual deployment of a selected revision unavailable without a source executor** |
| Superseded records | engine queue only | **Offline** for held events (`newer_selected`, `not_chosen`); queued-request supersession still not recorded by any CLI path |
| Live acceptance of pause/resume | — | **Pending** (pauses were seen live in L2–L4; held events and resume have no live scenario and need none beyond a status read) |

The B13 acceptance matrix row "Events held while paused; explicit `resume`" moves from **None** to **Offline (decision
state)**, with the boundary above. No other row changes; no criterion is upgraded to live by this PR.

## 8. Remaining Milestone B acceptance blockers

1. **Live runs not done:** L5 (runtime variables), L6 (preliminary failure with a running release), L7 (state-loss
   visibility), P1 (build overhead ≤ 1.2×), P2 (no resident process).
2. **Recovery edge outcomes** (`restored_unverified`, `restore_failed`, `not_attempted`) are offline-only; no natural
   application failure has been exercised live.
3. **Experimental live wiring versus the shipping gate:** the proven live replace, rollback, restore and the fault and crash
   injections exist only on local, unpushed branches; the shipping `replace`/`rollback`/`restore` remain simulate-only.
   Milestone B cannot close until the acceptance-proven wiring becomes a reviewed change behind an explicit gate decision.
4. **Owner decision:** whether "events held while paused / `resume`" is accepted at the durable-decision level for
   Milestone B (this PR) with execution deferred to Milestone D.

## 9. Smallest final verification campaign (each needs its own authorization)

1. **Prepare, offline:** land the gated live wiring as a reviewed change, rebuild one binary, record its SHA-256.
2. **One combined live session** (fresh application plus a bystander, one runtime directory, dry-run-first cleanup,
   inventory compared after the forwarder settles): L5 `--env` with an empty value and a value containing `=` through a
   replace, a restoration and a rollback, checking `docker inspect` arrays and the journal; L6 a candidate that fails its
   preliminary check while the previous release runs, checking `StartedAt`/id unchanged, no pause, the printed next actions;
   L7 the runtime directory moved aside (never deleted), `status --docker-socket` listing the labelled containers, then restored
   and the journal hash compared.
3. **P1** build overhead (one context, alternating direct and engine builds, N=5, medians) and **P2** (`ps` after each
   command shows no resident process), as written in `DEPLOY_B13_ACCEPTANCE.md`.
4. **Optional:** one natural (not injected) failing application for a non-`restored` recovery outcome.

Then a written acceptance review against the matrix.
