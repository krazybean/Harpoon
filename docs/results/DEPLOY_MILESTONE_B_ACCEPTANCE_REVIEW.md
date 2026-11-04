# Milestone B written acceptance review: reconciliation of the preserved Nov 2 evidence

Date: 2026-11-03 (sections 1–12), 2026-11-04 (sections 13–14). Status of sections 1–12: **evidence review and plan only.** Nothing was run,
restarted, deployed or changed when they were written; the journals were copied to a scratch directory and read with `sqlite3 -readonly`.
Section 13 records the isolated acceptance session that was then run. Section 14 records the gate-opening change that follows from it and
the owner risk decision it needs. Where sections 1–12 say the gate "is closed", that was true on 2026-11-03.

Reviewed against: `DEPLOY_B15_LIVE_INTEGRATION.md` (§4 acceptance executable and the evidence it must record, §7 gaps, §8 live
plans), `DEPLOY_MILESTONE_B_FINAL_ACCEPTANCE_RUNBOOK.md` §4 (what each step must show), `DEPLOY_B13_ACCEPTANCE.md` §4.

## 1. Owner decisions recorded with this review (2026-11-03)

1. `restored_unverified`, `restore_failed`, `not_attempted` and the remaining destructive crash scenarios are accepted on
   **offline evidence only**, as documented residual risks (section 8).
2. The idle-host precondition applies to **P1 (performance) only**. It no longer gates L5, L6, L7 or P2.
3. **P2** means: no `harpoon-deploy` process persists after a command (observed with `ps`, 60 s). The desktop's idle CPU and
   resident memory are measured separately and are not part of P2's pass rule.
4. **Gate U (unattended Git deployment) stays closed.** Nothing here changes it.

These decisions do not authorize opening `Gate::shipping()`.

## 2. Evidence sources, integrity and provenance

Preserved copies (read-only reviewed), root `~/harpoon-final-evidence-20261103/`:

| Item | SHA-256 |
|---|---|
| `canary-20261102.MANIFEST.sha256` | `bf975fffe3690e625b5a06ca67fdee4223e4b216fa105ad437964620f63d0911` |
| `canary-20261102-restore.MANIFEST.sha256` | `cf1e064dd79881482e664bf0a9f15908f4e4fc52f2a53d166c09d0089f5932c7` |
| `canary-20261102/runtime/deploy-journal/deployments.sqlite3` | `f3a9327d8f6b64b67754a812fcd181c09865421f6678bbc3158e3750e20c7f84` |
| `canary-20261102-restore/runtime/deploy-journal/deployments.sqlite3` | `efd5c8ab58576cdf50b99505893ccffcdbe0a24fa36066ac053776390b343dc7` |

**Manifest verification (`shasum -a 256 -c`, run inside each copy): `canary-20261102` 80 of 80 OK, `canary-20261102-restore` 58 of
58 OK. Every file on disk is listed in its manifest and no listed file is missing.** Both journal copies pass
`PRAGMA integrity_check` (`ok`). The two journal hashes equal the "before cleanup" hashes recorded in
`f3-journal-hashes-before.txt`, and `s4-journal.copy` and `s5audit-journal.copy` equal the preserved runtime journal of run 1.

The **original** directories under `~/harpoon-canary-acceptance/` are no longer evidence: their journals differ from the copies
because the 2026-11-03 F3 cleanup (`reconcile`, `teardown`, `cleanup-image`, each dry run first) was applied to them. Their
`evidence/` directories are identical to the copies. This review uses the copies only.

Files relied on, all under the copy root (SHA-256):

| File | SHA-256 |
|---|---|
| `canary-20261102/evidence/s1-deploy.stdout` | `6131b177ba9436086d15136bf94fdd4fc6f6eb06fd16bbbca4308e0e6cc1bbe9` |
| `…/s2-replace.stdout` | `991c711130d59b9e9e4747bc127191c50bc81080f99fe0c3682442530cdbfdf0` |
| `…/s3-replace.stdout` | `5d8a571629337d2f319f8d366e01488f7fb0597f1b54c66ecc27fc979527d845` |
| `…/s4-rollback.stdout` | `3cc43b664f5eb58c6df4f22b86e570c6c448a939732b74223f37f6e0cf5a7aef` |
| `…/s3-journal.copy` | `4db42aad3a9e624df7707232642c8ba43f37b17bec51603cb4a05dab0e5bd5ad` |
| `…/probe-s1.jsonl` | `c8e05045051ba0bde9873acb66a2f4a18bf87e4bcf5baebd5798db570ea767ed` |
| `…/probe-s2.jsonl` | `ec54486dd40696fff9d56c9523128b4bb162a59d3485551eb0c79ea569105e40` |
| `…/probe-s3.jsonl` | `0bb6f5674d55553fee1748bca5560e0d4addddb7b2d05c48aede2b357a6f8681` |
| `…/probe-s4.jsonl` | `b568608d9bea842e72af05e4f0f618bb154c7dfe79294f3c03ff66e1dd76468c` |
| `…/40-B-identity.txt`, `50-B-identity.txt` (identical) | `1a90955a168ebd12e593eba88e65e3c5aa003c15b46b11799cbaaafc5bc364b1` |
| `…/40-containers.txt`, `50-containers.txt` (identical) | `b185b8747b069cd7e46d23888edbd1a91cf11888a2970737300178a9ed77757c` |
| `…/40-images.txt` / `50-images.txt` | `25c67d23…` / `d286046a3c345f4f23dba6baa10384e2b1acbaa2bf3ebb5e73feb62070702b18` |
| `canary-20261102-restore/evidence/e2-deploy.out` | `d5646e1a10617ae0ad92904d6c7e65a863dec7412c74f833e2673ae00bafe6c5` |
| `…/e2-container-inspect.txt` | `1fc9a95a23d2a34407f3757ee1db93dc6a9178c92599ab63679d5594c7af26b4` |
| `…/e3-replace.out` | `9cb921a5a039453322724d147f02bd070432bc9690d7fac5830e3bbf24fb8fa9` |
| `…/e3-inspect.txt` | `1fd37615b738d2cadbb61d9ac492c7a40f48ee71ee95b3fb7e6e167710a3a365` |
| `…/e3-status.txt` (equals `f3-status-20261102-restore.txt`) | `86e5b837e4df978e1355becf9d2f304ffee82d8d6b406928f674e0ade5a55241` |
| `…/e3-probe.jsonl` | `73043d08c47bc60be55da0b3ba6c41c6c420536248f142875f5e353a1a6e1ca0` |
| `…/e3-containers.before` / `.after` | `c6a3dce7…` / `1d82c0d925c77cbbb7478c6c360f4eaca9b9b5a9e43b3c0813af30220f2c269f` |
| `f3-status-20261102.txt` | `c07dbe8d5b7ae9f11b1e30ba2ccd486ff7b18661d11638e4b84380751a8ce285` |
| `f3-apply-6-cleanup-image-ESHZ.txt` | `444638c977c16c4190f3abf867fb2d7c07c354916ef5fd9f1f1a758564bc9fff` |

**What the two runs were**

| Run | Time (UTC) | Binary as printed by the tool | Scenario |
|---|---|---|---|
| `20261102` (run 1) | Nov 2 18:40–19:00 | acceptance build, revision `37d8a6d0dc3b` (= #151) | S1 deploy A1, S2 healthy replace to B, S3 replace with a candidate that fails its preliminary check, S4 rollback to A1 |
| `20261102-restore` (run 2) | Nov 3 01:59–02:02 | **local fault-overlay build**, revision `b6b85f519486` (an unpushed overlay commit on top of #153) | e2 deploy A (r1), e3 replace to B (r2) with an injected final-check failure, then the engine's automatic bounded restoration |

**Provenance gaps (they apply to every row below):**
- **No binary SHA-256, toolchain version or `--acceptance-info` output was preserved for either run.** `DEPLOY_B15_LIVE_INTEGRATION.md` §4
  says to record the revision, the SHA-256 and the toolchain with the evidence. Only the revision printed in each run's banner survives.
- **Run 1 predates #153; run 2 includes #153 but is an overlay build.** Since `37d8a6d`, the deploy crates changed only in #153
  (`deploy-docker/src/backend.rs`, +49 lines, plus tests) and #156 (a new `deploy/src/route.rs`, +218 lines, read-only routing).
  Neither the current `main` nor #157 has been exercised against the deployment engine live.
- The Docker request log the runbook asks for (`docker-requests-note.txt`) was not preserved.

## 3. L5: environment pinning through deploy, replace, restoration and rollback

Criterion (B15 §8, runbook §4): each release is pinned with its own environment, and a restoration or rollback reproduces that
release's own pin, not the command line's or another release's.

**Proven by preserved evidence**

| Fact | Evidence |
|---|---|
| Each release is pinned as a `v2` pin with sorted variable names, and the pin differs between releases | Journal records (decoded): run 1 `…KK424BQ` (A1) `CANARY_BUILD=a1, CANARY_VERSION=A`; `…QNT1MD` (B) `b1, B`; run 2 `…5PXS0VYF` (A) `r1, A`; `…DH9AQZE` (B) `r2, B` |
| A rollback uses the target release's pin and says so | Journal: the rollback deployment `…XRK5` is pinned `a1, A`, equal to A1's pin, not B's. `s4-rollback.stdout`: "environment (pinned by release …BQ; not taken from this command)". |
| A replacement whose environment differs prints that it differs | `s2-replace.stdout`, `s3-replace.stdout`: "the candidate gets exactly the variables given now; a restoration or rollback uses each release's own pin" |
| **`docker inspect` `Config.Env` of the real containers (run 2)** | A: `CANARY_BUILD=r1 CANARY_VERSION=A HARPOON_DEPLOYMENT_MARKER=83a5eade…`. Candidate B: `r2, B, marker 23887823…`. **Restored container `…-restored`: `r1, A, marker af6606e0…`** (A's pin, not B's, and a new marker). Label `io.harpoon.restores` names A's deployment. (`e3-inspect.txt`) |
| Every serving container carries its own marker | Run 2 inspect: three distinct markers. HTTP `/health` markers on the serving ports equal the `Config.Env` markers of the containers behind them (`e3-probe.jsonl`: 83a5… on 38004 before, 23887823… on 38005, af6606e0… on 38004 after). Preliminary containers have their own markers too (`e611865c…` for run 2's B preliminary). |
| The application reports its environment in `/health` | In run 2, `build`/`version` in the response equal `CANARY_BUILD`/`CANARY_VERSION` from `docker inspect` for the same container. |
| Exactly one restoration, automatic, within budget | `e3-replace.out`: "restored … in 1496 ms (budget 60000 ms)"; no second attempt; no `restore --apply` in either run. |
| A failed cutover pauses the application, with reason, and no `resume` was run | `e3-status.txt`: PAUSED "failed its cutover (recovery restored)", held events 0. Run 1 `s4-rollback.stdout`: "recorded 1 pause record(s)"; `f3-status-20261102.txt`: PAUSED after the rollback. |

**Indirect evidence (inference, labeled)**
- Run 1 has **no `docker inspect` capture**. For the rolled-back container the environment is shown by the journal pin (`a1, A`) and
  by `/health` answering `version=A build=a1` with a new marker (`2bd52f90…`, not A1's `8258aa2f…`). Because run 2 shows the app echoes
  its environment, this is consistent with the right environment, but run 1 does not prove `Config.Env` directly.

**Gaps the preserved evidence cannot establish**
1. **An explicitly empty value (`KEY=`) and a value containing `=`** were never used live. All live values are plain (`a1`, `A`, …).
   Offline tests cover both (B13 §1.1).
2. **The complete runbook S4–S7 chain on one application** (replace, injected failure and restoration, then rollback to the first
   release) did not happen. The pieces were observed in two separate runs and two applications.
3. **Rollback after a restoration** was not run, and neither was **a failed replacement after a rollback with the #153 fix**
   (the defect that #153 repaired, "`not_attempted (config_unavailable)` when a rollback is serving"). That path has offline tests only
   (`deploy-docker/tests/b11_recovery.rs`, `deploy/tests/rollback.rs`). Run 2's journal has no rollback.
4. Two variables per release, not three, and no journal `v1` pin in a live journal.
5. Binary provenance (section 2).

**L5 classification: PARTIAL.** Restoration and journal pinning are proven live with `Config.Env`. Rollback is proven from the
journal and `/health`, not from `docker inspect`. Empty and `=`-containing values are offline only.

## 4. L6: preliminary health failure while a previous release is serving

Run 1, S3 (deployment `dep_01M6ESHZ0SRX1BKY9Y1E3CNR0K`, 18:50:11 to 18:51:13). The failing candidate was a **natural** failure,
an application answering HTTP 503, not an injected fault: `probe-s3.jsonl` shows 236 responses of 503 from the candidate's
preliminary port (39000).

| Required by runbook §4 | Result | Evidence |
|---|---|---|
| Command failed at the preliminary check, no cutover | **Verified** | exit 7; steps: `preliminary_check Failed`, `cutover Skipped`, `final_check Skipped`, `recovery Skipped`; "timed out after 60242 ms (budget 60000 ms)… 117 attempts, 0/2 consecutive successes… HTTP 503" |
| Serving container identity unchanged | **Verified** | `40-B-identity.txt` and `50-B-identity.txt` (before S3 and before S4) are byte-identical: id `0742ef670af1…`, started `2026-11-02T18:46:55.436Z`, running, restarts=0 |
| `StartedAt` unchanged | **Verified** | same files |
| Container inventory unchanged | **Verified** | `40-containers.txt` and `50-containers.txt` are byte-identical (no container created or removed); the only difference in images is the retained candidate image `sha256:18038d2b…` |
| No downtime of the serving release | **Verified** | `probe-s3.jsonl`: the serving port (38002) answered HTTP 200 on 282 of 282 probes during 18:50:09–18:51:20 |
| Candidate preliminary container removed | **Verified** | the inventory shows no extra container; the ledger shows `-pre` "released" (`f3-status-20261102.txt`) |
| No cutover or recovery record | **Verified** | `s3-journal.copy` (integrity ok): the deployment's snapshot rows end `failed` with the cutover field `not_started` |
| No pause | **Verified** | `s3-journal.copy` has 0 `control` records; output: "automatic triggers are not paused by this failure" |
| Printed next actions are real commands, with no `--simulate` advice | **Verified** | `s3-replace.stdout` lines 24–30: `status`, a retry via `replace`, and `cleanup-image … (a dry run; add --apply, …)`. No `--simulate` text. |
| Retained image removed only by the supported dry-run-then-apply path | **Verified** | `f3-teardown-dry.txt` (dry run) then `f3-apply-6-cleanup-image-ESHZ.txt` (removed by id, force=false) |
| No `stop`/`kill` request for the serving container in the Docker request log | **Not directly established** | the log was not preserved. Unchanged id, `StartedAt`, `restarts=0` and zero failed probes make a stop impossible to have gone unnoticed, but it is inference |

**L6 classification: PASS (verified), with the Docker-request-log item recorded as a provenance gap and the section 2 caveats.**

## 5. Restoration experiment: actual live coverage and limits

What ran (run 2): deploy A (`r1`) then `replace` to B (`r2`) with `--experimental-fail-final-check` from the local overlay build.
Preliminary check, stop of A, start of B and the restoration are real; only the final health check of the candidate was replaced by
an injected failure (`e3-replace.out`: "FAULT INJECTION ACTIVE").

**Covered live:**
- Outcome **`restored`** after a failed final check: the previous release restarted from its pin, passed its own health check in
  1496 ms, and the application answered again (`e3-probe.jsonl`). Total interruption 9809 ms.
- Restored container: A's image, A's environment, A's port (38004), A's labels plus `io.harpoon.restores`, a fresh marker.
- The failed candidate and its resources stayed recorded; `status` reported 2 unresolved cleanup items and a PAUSED application;
  `reconcile` dry run then `--apply` (F3, Nov 3) removed only the candidate's verified container and lease.
- Run 1 contributes nothing here: it had no failed cutover.

**Not covered, and why it matters little or can't be fixed by more testing:**
- **Not a natural failure.** The final-check failure was injected, so the engine's reaction to a real application failing only the
  final check is not shown. The runbook itself says an application cannot distinguish the preliminary container from the final one,
  hence the overlay.
- **Overlay build, not the acceptance build or the shipping code path.** The overlay differs from main by the injection only,
  but its binary hash was not preserved.
- **No rollback preceded the failure.** The #153 post-rollback case is offline-tested only.
- `restored_unverified`, `restore_failed`, `not_attempted`: no live evidence (owner-accepted residual risk).
- Crash during restoration: not exercised live (L4 covered one other boundary).

## 6. Requirement-by-requirement table

Classes: **VERIFIED** live evidence meets the check · **PARTIAL** live evidence meets part of it · **OFFLINE** offline suites
only · **MISSING** no evidence.

| # | Requirement | Class | Evidence | Remaining |
|---|---|---|---|---|
| 1 | Build, run, `--env`, preliminary check before replacement | **VERIFIED** (L1–L4 and runs 1–2) | `s1`/`s2` stdout, journal pins | empty / `=` values live: offline only |
| 2 | Replacement reports interruption beforehand and records measured duration | **VERIFIED** | "anticipated interruption…" printed; measured 1585 ms (S2), 1505 ms (S4), 9809 ms (run 2) | none |
| 3 | Preliminary failure leaves the previous release running, retains the image, lists real next actions | **VERIFIED** (L6) | section 4 | request log not preserved |
| 4 | `restored` | **VERIFIED** (L2, L4, run 2) | section 5 | injected, not natural |
| 4 | `restored_unverified`, `restore_failed`, `not_attempted` | **OFFLINE** | B13 §2 test list | owner-accepted (section 8) |
| 5 | Rollback with the pinned configuration, data/schema warning | **VERIFIED** (L3, S4) | `s4-rollback.stdout`: pinned image id, pin message, warning, 1505 ms | rollback `Config.Env` not inspected |
| 6 | History survives restart and `kill -9`; recovery offered | **VERIFIED** for one boundary (L4); others **OFFLINE** | `DEPLOY_MILESTONE_B_L4_RESULTS.md` | owner-accepted |
| 7 | Deleting the store leaves workloads; `status --docker-socket` reports them | **OFFLINE** (PR #130) | no store was moved live in either run | **L7 live: MISSING** |
| L5 | Environment through replace, restoration, rollback | **PARTIAL** | section 3 | section 3 gaps |
| L6 | Preliminary failure with a running release | **VERIFIED** | section 4 | none beyond provenance |
| L7 | Store loss visibility, read-only discovery, corrupt copy fails closed | **MISSING live** | offline tests only | one read-only live check |
| P1 | Build overhead ≤ 1.2× a direct build | **MISSING** | none | one measurement (idle host) |
| P2 | No lingering `harpoon-deploy` process | **PARTIAL** | by construction; B13 offline observation. No `ps` evidence in either run. | a 60 s `ps` observation |
| P2′ | Desktop idle CPU / resident memory (new wording) | **MISSING** | none | one measurement, separate from P2 |
| – | Bystander untouched | **VERIFIED** in run 1 | bystander container `2f70855f…` running in all four inventories (S1–S4) | – |
| – | Pause and held events behave per B14 | **PARTIAL** | pauses seen live after a failed cutover and a rollback; no event source | real delivery is Milestone D |
| – | Provenance: acceptance binary revision, SHA-256, toolchain recorded (B15 §4) | **PARTIAL** | revision in banners only | record it next time |
| – | `Gate::shipping()` decision and written review | open | this document is the review's evidence part | owner decision |

## 7. Defects found by this review

- **No engine defect.** The earlier live finding (the #153 post-rollback restoration) was already fixed and has offline tests.
- **Documentation/evidence defects:** the repo docs (roadmap, status, B15 §7) still say L6 and the restoration and rollback
  environment checks are untested, although the Nov 2 runs exist; the evidence lacks binary hashes and the Docker request log.
- **Design gap for the UI (not an engine defect):** see section 10.

## 8. Owner-accepted residual risks (offline evidence only)

1. `restored_unverified`, `restore_failed`, `not_attempted`: no live occurrence. Offline: `a_restored_release_that_never_passes…`,
   `a_restoration_that_cannot_start_the_container_is_restore_failed_and_not_retried`, `a_missing_previous_image_or_unreachable_docker…`.
2. Destructive crash points other than L4's single boundary, including a crash during restoration (offline crash-at-every-write suites).
3. VM stopped mid-deploy and disk full during a build (preflight offline).
4. A failed replacement after a rollback (#153) is verified offline only.
5. Empty-valued and `=`-containing variables live: offline only.

## 9. Minimal remaining live-test inventory

All of it runs on an **isolated engine** (`HARPOON_TEST_TMPDIR`), never the serving engine, needs its own written authorization, and
has not been started. Order: L7, P2 and the L5 inspect proof first (any host load), then P1 last (idle host only).

| Item | What | Pass | Stop |
|---|---|---|---|
| L7 | with one deployed app and a bystander: move the runtime directory aside (never delete), `status --docker-socket`, move it back; then a corrupted copy | warning printed, labelled workloads listed, containers unchanged, journal hash equal before and after, corrupt copy exits 4 and the original is byte-identical | any container change |
| P2 | `ps` for `harpoon-deploy` after each command and for 60 s | none | – |
| L5-inspect | one application, `--env EMPTY= --env EQ=a=b --env MODE=one`; `docker inspect` the replacement and the rollback container | `Config.Env` equals each release's own pin plus its own marker | a mismatch |
| Provenance | record the binary SHA-256, `--acceptance-info` and toolchain | recorded | – |
| P1 | runbook §5 | B/A and C/A ≤ 1.2 | inconclusive is not a pass: record the numbers and let the owner decide |
| P2′ | sample the desktop's idle CPU and RSS | recorded, no threshold imposed by this review | – |

Optional (not required by the owner's decisions): a rollback followed by a failed replacement, to confirm #153 live.

## 10. UI authorization: the minimum design for ordinary deployment (design only, not implemented)

**What the code does today (verified in source).**
- The desktop's **Deploy** runs `git-deploy --repo …` (`deploy_api.rs:1217`). That runs `commands::deploy` for a first release and
  `commands::replace` when the application already has a serving release (`git_cmds.rs:516`).
- `commands::deploy` live path requires the open `live_deploy_enabled` gate **and** `--authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE`
  (`commands.rs:1628–1634`). `commands::replace`, `rollback` and `restore` require `live_replacement_enabled` and the same flag
  (`commands.rs:1796–1803`).
- The desktop passes the socket, management socket, `--resources-dir`, `--forwarder-pid` and `--forwarder-command`, but **never the
  authorization flag**, and a test pins that (`deploy_api/tests.rs:448`).
- It never passes `--confirm-cutover`. A replacement started from the UI therefore only builds and checks a candidate and stops
  (git_cmds.rs:503 only forwards the switch if the caller gave it).
- `gate_open_for` checks `live_deploy_enabled` for `Deploy`, but a Deploy of an already-served application takes the replacement
  path and needs `live_replacement_enabled` too.

**Consequence.** Flipping `Gate::shipping()` alone leaves every UI deploy refused with exit 6, and a UI replacement would never cut over.

**Decision needed from the owner.** Whether the in-app confirmation is the authorization.

**Proposed minimum behavior (smallest change that keeps the opt-in an explicit user act):**
1. A confirmation dialog, shown before every operation that changes Docker (Deploy, Rollback, Restore), stating in plain words
   what will change: for a first deployment that Harpoon will build an image and start a container; for a replacement the
   anticipated interruption and that the application is down from the stop (taken from the plan the engine already prints); for a
   rollback, the existing data/schema warning. The existing `confirmAction` component is the likely vehicle.
2. Only on the user's confirmation does the backend add `--authorize-live-docker-changes=<phrase>` (and, for a replacement, `--confirm-cutover`).
   The phrase lives in one Rust constant shared with the CLI's `AUTH_PHRASE`; the front end never holds it and cannot send it alone.
3. The Tauri command takes an explicit `confirmed: true` per call. No "remember my choice", no flag in settings, and **no
   confirmation path for `Poll`** (it stays `watch --once --queue-only`, never deploys).
4. `RunQueue` is a user-initiated button; it needs the same dialog. Nothing runs on a timer or on poll.
5. Tests: invert the existing assertion to "the flag is added only when `confirmed` is true"; add one that no code path other than a
   confirmed call adds it; add one that `Poll` never does; add a guard that no desktop code calls `watch` without `--queue-only`.
6. Unchanged: the CLI keeps requiring the flag. Gate U stays closed (no background execution is added).

**Open question for the owner:** whether a first deployment needs both gates opened (deploy-only could be staged first, but a second
deploy of the same app would then be refused at the replacement gate).

## 11. Verdict of this review

Milestone B live evidence is **stronger than the repo docs state** (L6 and most of L5 exist), and **L7, P1 and P2's `ps` observation
are still missing**. Section 9 is the complete remaining live inventory. Opening `Gate::shipping()` also requires the
authorization design in section 10, which the gate flip alone does not provide. This review changes no gate.

## 12. Planned isolated acceptance session (PLAN ONLY: not authorized, not run)

Prepared 2026-11-03 after the owner's closure decision. Sections 1–11 above are unchanged; version 1 of this document (sections 1–11)
had SHA-256 `4bc0637b9d0fddfbb6939a5952672f2ea08efe746907165ace33ad9588e532b3`.

**Owner decision carried into the plan:** the production PR enables both attended initial deployment and attended replacement, adds a
desktop confirmation flow (section 10), keeps explicit authorization for real Docker changes and keeps Gate U closed. This session does
not implement any of that and does not open `Gate::shipping()`.

### 12.1 Scope (and nothing else)

| ID | Closes | Out of scope |
|---|---|---|
| L5 | environment pinning through deploy, replace, restoration, rollback, **including an empty value and values containing `=`**, with `docker inspect` `Config.Env` for every container | L6 (done), crash points, the three accepted restoration outcomes |
| L7 | read-only visibility with the journal missing; a corrupt copy fails closed | any recovery or repair |
| P2 | no lingering `harpoon-deploy*` process; desktop idle CPU/RSS measured separately (P2′) | any threshold on P2′ |
| P1 | the prescribed N=5 benchmark (runbook §5), last, on an idle host | tuning, reruns to improve a number |

### 12.2 Binaries, source and provenance to record (fixes the Nov 2 provenance gap for this session)

| Binary | Built from | Purpose |
|---|---|---|
| `harpoon-deploy-acceptance` | `main` `e964398` (tree `9ab42fc4de62fd52b8fd9b5447252931d19bf0e4`), `--features acceptance`, `--offline --release` | every live step except S3 |
| `harpoon-deploy-b15-overlay` | the same tree plus the **local, unpushed** overlay patch (below) | **only S3** |
| `harpoon-deploy` (shipping) | same tree, no features | proves the shipping gate still refuses `deploy`, `replace`, `rollback`, `restore` (exit 6) |
| engine under test | a copy of the current `harpoon/build/harpoon`, SHA-256 `711581b65c261a794ace1365c35935d56ac1fd703084267e078ee252cddbd877` (contains #157), placed at a scratch path **named `harpoon`** | the isolated engine |

- Overlay patch: `git diff $(git merge-base overlay/restore-final-check-fault-4af33a8 main) overlay/restore-final-check-fault-4af33a8`
  (merge base `713297f`), 8 files, 509 insertions, saved as `overlay-full.patch`, SHA-256 `83b9af0b567771dfcb2b5aa82d737b41d90f0abff675a8d9c426a95c7bc6d7f6`.
  `git apply --check` against current `main` succeeded (nothing modified). Its content is exactly `--experimental-fail-final-check`
  (`overlay.rs`, `overlay_main.rs`, plumbing and a test); it contains no crash or kill control. The branch stays local and unpushed;
  the main-branch guard tests fail on it by design (the tripwire).
- Built from an extracted archive of the commit (`git archive`), so no repository state changes. A different path gives a different hash
  (B15 §4), so build once at a fixed path and record that hash. `HARPOON_ACCEPTANCE_REV` and `HARPOON_OVERLAY_REV` are set explicitly
  because an archive has no `.git`.
- Toolchain on this host today: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, `cargo 1.97.1 (c980f4866 2026-06-30)`, macOS 27.0.1 (26A434).
  The F4 note records that release builds can fail here on proc-macro stripping and were worked around with
  `CARGO_PROFILE_RELEASE_STRIP=none` (an environment variable, no config change). If the build needs it, it is recorded with the hashes.
- Recorded before any live step into `00-provenance/`: commit and tree hash, `Cargo.lock` SHA-256, `rustc -V`, `cargo -V`, `sw_vers`,
  every environment variable used by the build, SHA-256 of each binary, `--acceptance-info` and `--overlay-info` output, SHA-256 of
  the overlay patch, and the engine copy's SHA-256 and `codesign -dv` identifier.

### 12.3 Test fault injection (declared before execution)

Exactly **one**: S3 injects a failure of the candidate's **final** health check (`--experimental-fail-final-check`, overlay binary only).
It is needed because L5's restoration step needs a failed cutover, and an application cannot tell its preliminary container from its
final one (identical image, env and in-container port). Preliminary check, stop, start and restoration remain real. No crash,
`SIGKILL` or synthetic preliminary failure is used.

- Rejected alternative: a "natural" failure through an external control sidecar (flip the candidate's state after the preliminary check).
  It needs a cross-container control channel and has timing risk. It would be new test machinery for a one-step need.
- Owner option: drop S3 and S4. Restoration then stays covered by run 2's plain-value proof (`r1/A` restored, own marker) and L5 would
  close on deploy, replace and rollback with empty and `=` values. **Recommendation: keep S3**, because the owner scoped restoration into L5.

### 12.4 Isolation of the serving engine and Docker resources

The isolated engine runs in the supported test namespace: `HARPOON_TEST_TMPDIR=/tmp/hpacc` (short, unix-socket limit), which gives its
own lock, lifecycle lock, docker and management sockets, stop file, log and disk (#155; `regression-lifecycle-isolation.sh`). VM sizing:
`--cpus 2 --memory 2048 --disk-size 8G` (sparse). Every item below is a **gate**: a failure stops the session before the next step.

| # | Check | Pass |
|---|---|---|
| I1 | Serving baseline captured: PID 8964, elapsed time, engine binary SHA-256 (`711581b6…`), `/tmp/harpoon-docker.sock` identity, `docker --context harpoon` container list (10 containers, none running), images (8), volumes (1), listeners | recorded; re-compared before every phase and at the end; any difference is a stop |
| I2 | `HARPOON_TEST_TMPDIR` is set **only on the commands that target the isolated engine** (per-command `env`, never exported in the working shell; this caused a confusing status read earlier) | enforced by a wrapper |
| I3 | `harpoon status --json` in the namespace: `isolated: true`; `socketPath`, `lockPath`, `lifecycleLockPath`, `controlSocketPath`, `mgmtSocketPath`, `stopFilePath`, `logPath` all start with `/tmp/hpacc/`; none contain `Library/Application Support/Harpoon` | all seven |
| I4 | The isolated engine's PID is not 8964; its binary path is the scratch copy; `lsof -p <pid>` shows only sockets under `/tmp/hpacc` | true |
| I5 | Every Docker and CLI call names the isolated socket explicitly (`-H unix:///tmp/hpacc/harpoon-docker.sock`, `--docker-socket`, `--mgmt-socket`). `DOCKER_HOST` unset, `docker context show` unchanged (`harpoon`), `DOCKER_CONFIG` an empty scratch directory for builds | wrapper refuses any other socket path |
| I6 | `--forwarder-pid` is the isolated engine's PID and refuses 8964; `--forwarder-command harpoon` (the copy is named `harpoon`) | the engine's forwarder-ownership proof passes for the right PID only |
| I7 | `--runtime-dir` is a fresh directory under the session's evidence root, never `~/Library/Application Support/com.harpoon.desktop/deploy` | path guard |
| I8 | Port ranges: no listener in 38000–38999 or 39000–39099 at start | none (the serving engine has no deployed application) |
| I9 | All containers and images the session creates are session-owned: own application ids, or the explicit bystander; removed by exact id after a dry run; no prune, no force, no `rmi -f` | true |
| I10 | The evidence-root guard from the runbook (`case "$L" in …`) and refusal to write into a non-empty step directory | true |
| I11 | Host headroom: free disk ≥ 8 GiB at start (**today 11 GiB at 98 % used, so close to the limit**), free + inactive memory ≥ 4 GiB; stop if free disk < 4 GiB at any step | recorded each step |
| I12 | Shipping gate: the shipping `harpoon-deploy` exits 6 for `deploy`, `replace`, `rollback`, `restore`; the acceptance binary rejects both `--experimental-*` flags (exit 2) | exact exit codes |
| I13 | End: serving engine PID 8964 still running with a consistent elapsed time, same inventory, same binary hash, `harpoon doctor` unchanged; the isolated engine stopped with `HARPOON_TEST_TMPDIR=/tmp/hpacc harpoon stop` after I3/I4 re-check, logs copied, then `/tmp/hpacc` removed only after `realpath` equals `/tmp/hpacc` | true |

The corrupt-copy step in L7 flips a byte in a **copy** only; the original journal is hashed before and after.

### 12.5 Order, actions, expected Docker mutations (isolated engine only)

Fixture: one image, `FROM alpine:3.22` pulled into the isolated engine only (Harpoon builds with `--pull=false`). The app is a small
busybox `nc` server (the pattern `regression-loopback-publish.sh` already proves) answering `/health` with JSON listing `EMPTY`
(`${EMPTY-<unset>}`, so empty and unset differ), `EQ`, `MODE`, `NEW` and `HARPOON_DEPLOYMENT_MARKER`. Port numbers are read from plan
output, never assumed. Variables come from `$L/vars.sh`.

| Step | Command (binary) | Docker mutations expected |
|---|---|---|
| B0 | build the three binaries and record provenance (host compile, no Docker); start the isolated engine; check I1–I12; pull `alpine:3.22` into the isolated engine | none on the serving engine |
| S1 | `deploy` A1: `--env EMPTY= --env EQ=a=b --env MODE=one` (acceptance) | image, preliminary container (created and removed), serving container, lease |
| S2 | `replace` A2: `--env EMPTY= --env EQ=x=y=z --env MODE=two --env NEW=2 --confirm-cutover` (acceptance) | image; preliminary create/remove; stop A1; create+start A2 |
| S3 | **overlay** `replace` A3: `--env EMPTY= --env EQ=c=d --env MODE=three --confirm-cutover --experimental-fail-final-check` | image; preliminary create/remove; stop A2; start candidate; injected final-check failure; stop candidate; one restoration of A2 (`…-restored`) |
| S4 | `reconcile` dry run, then `--apply` | remove only the failed candidate's verified container and lease |
| S5 | `rollback --to <A1 release> --confirm-rollback` (acceptance) | preliminary create/remove from A1's retained image; stop restored A2; create+start A1′ from A1's pin |
| S6 | L7 (below); needs a session-owned **bystander** container (plain `alpine sleep`, no Harpoon labels) created before S1 and left running | none |
| S7 | P2 observations (every CLI call, plus 60 s after the last) and P2′ sampling | none |
| S8 | P1 (runbook §5), last | per run: one image (removed by exact id) and, for Harpoon runs, a transient preliminary container |
| S9 | dry-run-first cleanup of session-owned resources by exact id; stop the isolated engine; I13 | remove only session-owned resources |

After every step: container, image and volume JSON from the isolated engine, the serving-engine baseline (I1), `ps` for
`harpoon-deploy*`, a copy of the journal, its SHA-256 and `PRAGMA integrity_check`, and `shasum -c` against a manifest written at the end.

### 12.6 PASS / FAIL criteria

**L5 (S1–S5). PASS only if all hold:**
1. For A1, A2, the restored container and A1′ (the rolled-back container), `docker inspect` `Config.Env` contains **exactly** that release's own
   variables as pinned (names and values byte-for-byte, including `EMPTY=` and the `=`-containing values), plus its own
   `HARPOON_DEPLOYMENT_MARKER`; image-defined variables (`PATH` …) are listed separately and ignored.
2. The **restored** container's pinned variables equal A2's, not S3's; A1′'s equal A1's, not A2's.
3. Every container ever created has a **distinct marker**, and each serving container's `/health` marker equals its `Config.Env` marker; the
   `/health` values for `EMPTY` (empty, not `<unset>`) and `EQ` equal the pin.
4. The journal copy decodes `v2` pins with names in sorted order and the exact values, one per release; the rollback deployment is pinned equal to A1's.
5. Exactly one restoration occurred; no `restore --apply` and no `resume` was issued; `status` shows one durable failed-cutover pause after S3 and a rollback pause after S5.
6. The bystander and everything present at B0 are unchanged (id, state, `StartedAt`).
7. The shipping binary still refuses the three live commands (I12) at the start and at the end.

**FAIL:** any pinned value differs (including an empty value turned into unset, or a value truncated at `=`), the restored or rolled-back
container uses another release's variables, a repeated marker, a second restoration, or a pin that does not decode.

**L7 (S6). PASS only if all hold:**
1. With the dedicated runtime directory renamed aside (same filesystem, never deleted), `status --runtime-dir <moved-away path> --docker-socket <isolated socket>`
   exits 0 and prints the "journal: not initialised … a store that was deleted or lost looks the same" notice and the "deployment history is unavailable" warning.
2. It lists every Harpoon-labelled container the session created, marked UNVERIFIED, and counts the bystander as "other, ignored".
3. Container ids, states and `StartedAt` are identical before and after the step; nothing was adopted, recorded or recreated; no journal file appeared at the moved-away path.
4. After renaming back, the journal SHA-256 equals the pre-move hash, `integrity_check` is `ok`, and ordinary `status` works.
5. A **copy** with one byte flipped inside the stored data makes `status --runtime-dir <copy>` exit **4**, print "Do not delete", and **not** fall back to Docker discovery; the original journal is byte-identical afterward.

**FAIL:** any file created in the moved path, any container change, discovery output for a corrupt store, an exit code other than 0 and 4, or a changed original journal.

**P2 (S7). PASS:** no process whose command line contains `harpoon-deploy` exists, nor any child of one, immediately after each CLI call and for 60 s after the last.
**P2′:** record, never judge: for the desktop process (see open question 3) `ps` %CPU and RSS every 5 s for 120 s, plus the state it was in. **FAIL P2:** any lingering process.

**P1 (S8).** Exactly the runbook's rules: three metrics (A direct, B internal build step, C end-to-end build path), N=5 per kind after one discarded
warm-up each, strictly alternating, raw CSV kept, nothing dropped. Verdict: **PASS** if all equivalence checks pass and B/A ≤ 1.2 and C/A ≤ 1.2;
**NOT PASSED** if B/A ≤ 1.2 and C/A > 1.2; **FAIL** if B/A > 1.2; **INCONCLUSIVE** if any equivalence check fails, a run exits non-zero, or the first
warm-up is outside 2–60 s. A result is never re-run inside the session. An INCONCLUSIVE or NOT PASSED verdict is reported with the numbers and decided by the owner.
**P1 idle-host precondition** (checked immediately before S8; the numbers are my proposal, the Nov 1 record gave none, only its failing sample of load 4.29/6.18/7.44, 38.67 % idle on 11 cores):
1-minute load average ≤ 2.0, CPU idle ≥ 85 % averaged over a 60 s `top` sample, no other process above 15 % CPU, VS Code, Chrome, Codex and the dev desktop's Vite server closed or idle. If it fails, **P1 waits**; L5, L7 and P2 are unaffected.

### 12.7 Stop conditions

Stop, preserve evidence, and run **no** reconcile, restore, retry, removal, engine action or VM action, on any of:
- any change to the serving engine (PID, elapsed time, binary hash, inventory) or to the bystander or any resource present at B0;
- any container, image or volume appearing or disappearing outside the step's expected row; a missing ownership label on a session-created resource;
- a second restoration attempt or any retry by Harpoon; an ambiguous serving identity in `status`; two containers claiming one port;
- a request on the serving socket from any session command; any session process holding a serving-engine path;
- an authorization or flag refusal that was not expected; an evidence-root guard failure; journal integrity failure;
- a command running longer than twice its printed budget; free host disk < 4 GiB;
- the isolated guest `dockerd` failing to start (the containerd pid-file flake seen in earlier throwaway VMs): **stop and report; no retry without further authorization**.

### 12.8 Authorizations the session needs (none granted yet)

1. Build the three binaries and apply the overlay patch **in a scratch copy**, `--offline`, no repository change.
2. Start and stop one isolated engine in `/tmp/hpacc`; pull `alpine:3.22` into it (network).
3. Real Docker changes **inside the isolated engine only**, with `--authorize-live-docker-changes=…` on each live command.
4. The one S3 fault injection.
5. Non-forcing, non-pruning removal by exact id of the session's direct-benchmark images.
6. A byte-flip on a **copy** of the journal.
7. Read-only sampling of one desktop process (P2′).
Estimated duration: about 1.5–2 hours, most of it P1.

### 12.9 What this session still cannot fix (unresolved provenance limitations)

- **The Nov 2 runs keep their gaps for ever:** no binary hash, toolchain or Docker request log was preserved, and run 1 predates #153. This session records its own provenance
  and exercises current `main` (`e964398`), but it does not retroactively attest the Nov 2 binaries.
- **No Docker request log mechanism exists in the product.** The runbook's `docker-requests-note.txt` cannot be produced. "No `stop` for the serving container" is
  shown by unchanged id, `StartedAt` and restart count, which is inference, as in L6.
- **The desktop UI path is not covered** (the authorization flow is not built). It needs its own pre-merge live proof with the PR's build (section 10).
- **Binary identity of the overlay** is an unpushed local build by design.
- **P2′ measures a debug build** if the already-running development desktop is sampled (open question 3).

### 12.10 Questions for the owner before approval

1. Keep S3/S4 (one overlay fault injection), or drop restoration from this session? **Recommendation: keep.**
2. Approve the proposed numeric idle-host thresholds, or give others?
3. P2′ target: the running development desktop (PID 30867, debug `tauri dev`, read-only sampling now) or the packaged app. The packaged app cannot run beside it (both claim port 7480, F4 finding), so it would be measured later after the dev app exits.
4. Host disk is at 98 % used (11 GiB free). Free some space first, or accept the 8 GiB sparse VM disk and the 4 GiB stop line?

## 13. Results of the isolated acceptance session (executed 2026-11-04 03:35–03:50 UTC)

Outcome: **PARTIAL.** L5, L7 and P2 PASS. P2′ recorded. **P1 did not run** because the agreed idle-host thresholds were not met. The serving engine
(PID 8964), its disk, Docker inventory, context and DNS were unchanged throughout. `Gate::shipping()`, Gate U and the desktop journal were not touched.

Evidence root: `~/harpoon-b-acceptance-evidence/L567-20261103/` (175 files, `SHA256SUMS` verified, 0 mismatches; SHA-256 of the manifest
`55d735623dedf4c4e42742fe695339ec7495649ad013fdb956168f625f64bec0`).

**Provenance (new session):** source `main` `e964398f8d5b19ba5b5183cdf0ec2d165e78d842`, tree `9ab42fc4de62fd52b8fd9b5447252931d19bf0e4`, built from `git archive`;
`deploy-cli/Cargo.lock` (gitignored; copied from the working tree) `1cae0d3468ca58901d4306e6d26c0f9b4576817074ce70c95b50a7263ddc3a75`; `rustc 1.97.1 (8bab26f4f 2026-07-14)`,
`cargo 1.97.1 (c980f4866 2026-06-30)`, macOS 27.0.1 (26A434), no build environment overrides (the `STRIP=none` workaround was not needed).

| Binary | SHA-256 |
|---|---|
| `harpoon-deploy` (shipping) | `2fc13c1431a4e037f8835bf32c540373571aa8391e680692250b9d410f1413a0` |
| `harpoon-deploy-acceptance` | `e7b08ca33339fa1ba918f8cfdf89e644efca9254534f35527c9ade69f09094b8` |
| `harpoon-deploy-b15-overlay` (main + overlay patch `83b9af0b…`) | `2689326cee29bb055a1442c48f1f7afe40fe046dfa0deedf264031e128be1cdc` |
| engine under test (copy of `harpoon/build/harpoon`, contains #157) | `711581b65c261a794ace1365c35935d56ac1fd703084267e078ee252cddbd877` |
| bundled Buildx used (copy, `resources/harpoon/bin/docker-buildx`) | `70dbccab5e0d261353a802315651f6827cee90bb22f5f3eeb39ffc7bbe55e075` |

Isolated engine: PID 77298, `HARPOON_TEST_TMPDIR=/tmp/hpacc`, 2 CPUs, 2048 MiB, 8 GiB disk. Gates passed: I1, I3 (all seven paths inside `/tmp/hpacc`), I4, I5, I6
(`--forwarder-pid 77298`), I7, I8, I12 (shipping binary: exit 6 for `deploy`, `replace`, `rollback`, `restore`; acceptance binary rejects both `--experimental-*` flags: exit 2).

| Requirement | Result | Evidence (under the evidence root) |
|---|---|---|
| **L5** env through deploy, replace, restoration, rollback, with `EMPTY=` and `=`-containing values | **PASS** | `S1d-after/checkenv.txt`, `S2-after/checkenv.txt`, `S3-after/checkenv-restored.txt`, `S3-after/checkenv-candidate.txt`, `S5-after/checkenv-rolledback.txt`, `S5-after/l5-summary.txt` |
| L7 lost store: status read-only, lists labelled workloads | **PASS** | `S6-L7/status-missing.txt` (exit 0; "journal: not initialised"; 4 Harpoon-labelled containers UNVERIFIED; 1 other ignored; nothing created at the moved-away path; ids, states, `StartedAt`, restarts unchanged; journal SHA-256 identical after moving back; `integrity_check` ok) |
| L7 corrupt copy fails closed | **PASS** | `S6-L7/status-corrupt.txt` (exit 4, "checksum mismatch", "Do not delete", no Docker discovery); original journal byte-identical |
| P2 no lingering `harpoon-deploy*` process | **PASS** | `S7-P2-P2prime/samples.csv` (0 in all 25 samples over 120 s) and 0 after each CLI call |
| P2′ desktop idle baseline (debug `tauri dev` PID 30867, read-only) | **recorded** | CPU: median 0.0 %, mean 0.36 %, max 2.7 %; RSS 35.6–35.8 MiB (a debug build, not the shipping package) |
| P1 build overhead | **NOT RUN** | idle gate failed: 1-min load 3.36 (needs ≤ 2.0), CPU idle mean 79.4 % over 60 s (needs ≥ 85 %), WindowServer at 46 % CPU (needs ≤ 15 %): `S8-P1-precondition/` |

**L5 detail (all five checks hold):**
- A1 pinned `EMPTY=`, `EQ=a=b`, `MODE=one`; A2 `EMPTY=`, `EQ=x=y=z`, `MODE=two`, `NEW=2`; the injected-failure candidate A3 `EMPTY=`, `EQ=c=d`, `MODE=three`. `Config.Env` and `/health` matched each pin exactly
  (`"empty":""`, not unset; `EQ` kept everything after the first `=`).
- S3 (one overlay fault): exit 7, `recovery: restored` in 1459 ms, exactly one restoration, one durable failed-cutover pause. The **restored** container carries **A2's** pin
  (`MODE=two`, `NEW=2`, `EQ=x=y=z`), not A3's, with a new marker. S4 `reconcile` dry run named exactly the failed candidate and its lease; apply removed only those.
- S5 rollback to A1: the rolled-back container carries **A1's** pin, printed "pinned by release …, not taken from this command", second pause recorded; measured interruption 1408 ms.
- Markers of all five containers created are distinct (`cf285391…`, `88146141…`, `55cb48f4…` candidate, `647f22a7…` restored, `04b1298d…` rolled back), and each serving container's `/health` and its response header equal its `Config.Env` marker.
- The journal decodes four `v2` pins with sorted names, matching the above (`S5-after/l5-summary.txt`).

**Deviations and honest notes**
- Three of my setup mistakes, each failing before any Docker change and each corrected once, none a product defect: `--resources-dir` must contain `harpoon/bin/docker-buildx` (`S1`, `S1b`); the first fixture did not return the
  required `X-Harpoon-Deployment-Marker` header, so its preliminary check failed after 60 s (`S1c`; its retained image was removed with `cleanup-image`, dry run first, then `--apply`). The accepted run is `S1d`.
- `status --json` created the isolated disk before `start`, making `--disk-size` conflict (the file was recreated by the CLI, inode distinct from the serving disk `360355014`); I removed only that namespace file after a resolved-path check and started again.
- Two `SERVING-CHANGED` flags came from my own checker (it counted the isolated engine's own forwarder listener, and later matched its own command line with `pgrep -f`). Both were investigated and re-checked: the serving engine was unchanged.
- "No process whose command line contains `harpoon-deploy`" would match the harness's own shell; P2 was measured by **executable name** (`ps … comm`).
- `plan` still prints "live execution: disabled in this build until B7" in the acceptance build. Cosmetic, stale text.
- Host free disk: 25 GiB at the start, 24 GiB (`df -g`) after the builds and for the rest of the session; never near the 15 GiB stop line. Build tree 150 MiB, evidence 72 MiB.
- Cleanup: the isolated engine was stopped, its namespace (including the 8 GiB sparse disk and all session-created Docker resources) removed after logs, journals and inventories were copied. Per-resource `teardown` was not run because the VM disk was discarded.
- The binary copies, build tree (`~/.harpoon-b-acceptance-build-L567-20261103`) and evidence root are retained.

**Remaining blockers for closing Milestone B live evidence:** P1 (needs an idle host); the desktop confirmation flow and its own pre-merge live proof (section 10); the owner's decision on the written acceptance review and `Gate::shipping()`.

## 14. The gate-opening change and the owner risk decision (this PR)

**Scope approved by the owner:** enable attended initial deployment and attended replacement; keep unattended Git-triggered execution (Gate U) closed;
keep the explicit CLI authorization for real Docker changes; add a desktop confirmation instead of bypassing it; keep acceptance-only binaries,
overlays and experimental flags out of shipping builds; keep reconciliation, rollback and ownership safeguards unchanged.

**What changes (details in the PR):**
- `Gate::shipping()`: `live_deploy_enabled: true`, `live_replacement_enabled: true`, and a new `live_triggered_enabled: false` (Gate U) that guards `git-run` and `watch`
  without `--queue-only`. `git-deploy` (a revision a person chose) now also refuses a missing opt-in before it contacts a repository. Only the unpackaged acceptance binary
  sets `live_triggered_enabled: true`; a packaging test pins that.
- Desktop: `deploy_op` takes a separate `confirmation` argument. The backend alone adds `--authorize-live-docker-changes=…`, only when the person confirmed that operation,
  and `--confirm-cutover` only for a replacement the person separately acknowledged. Cancel sends nothing. Failures carry a plain-language `hint` instead of an exit code.
- Unchanged: the exact CLI phrase, explicit socket/paths, forwarder-ownership proof, ownership labels, `reconcile`/`teardown`/`cleanup-image` dry-run-first, bounded single restoration.

**P1 is NOT RUN. It is not claimed as passed.** The build-overhead requirement (at most 1.2× a direct build) was not measured on 2026-11-04 because the host failed the
agreed idle thresholds (load 3.36 against ≤ 2.0, CPU idle 79.4 % against ≥ 85 %, WindowServer at 46 % CPU). No P1 number exists, so no performance claim about
Harpoon's build path may be made.

**Owner decision (2026-11-04): P1 is accepted as NOT RUN and deferred for this release candidate**, because the host failed the agreed idle-host precondition. This is a risk
acceptance, not a pass: P1 is not marked PASS, the 1.2x target is not claimed to have been met, and no statement about Harpoon's build overhead may be made. Conditions
that stay attached to the decision: (1) the release notes and this document say P1 was not measured; (2) P1 is run later on an idle host under the runbook's rules (a fresh
isolated engine, N=5, verdict rules fixed beforehand) and its result is recorded here whatever it is; (3) a result above 1.2x is a defect to track, not a retroactive pass.

**Residual risks carried (owner-accepted on 2026-11-03 unless noted):**
1. The Nov 2 canary evidence keeps its provenance gaps (section 2): no binary hash, toolchain or Docker request log; run 1 predates #153. The 2026-11-04 session
   re-proved L5 and the restoration with fully recorded provenance, but the Nov 2 binaries are not retroactively attested.
2. `restored_unverified`, `restore_failed`, `not_attempted`, other crash points, a VM stopped mid-deploy and a full disk: offline evidence only (section 8).
3. A failed replacement after a rollback (#153): live-verified only through the Nov 2 restoration (without a prior rollback) and offline tests.
4. **The desktop confirmation path has no live evidence yet.** The CLI path is live-proven; the desktop adds the dialog and the backend argument plumbing. A bounded end-to-end
   Harpoon Hello deployment from the desktop on an isolated engine is required before merge (see the PR's pre-merge steps). It has not been run.
5. **Trust boundary of the confirmation:** the dialog and the backend run in the same application. A compromised web view could send a confirmation without a person.
   The Tauri command accepts a confirmation only as a separate typed argument and only for Docker-changing operations, and the front end never holds the engine's phrase, but
   an in-process attacker is not defended against. Hardening (a native confirmation sheet, or a single-use token issued when a dialog opens) is not part of this change.
6. `rollback` and `restore` share the `live_replacement_enabled` gate with `replace`; they cannot be opened separately.
7. Executing queued Git events (`RunQueue`, `git-run`, `watch`) stays closed in every shipping build. Polling and queueing still work and deploy nothing.

This document does not declare Milestone B unconditionally accepted. L1–L7 and P2 have passing live evidence (L6 from the Nov 2 canary, the rest from the 2026-11-04 session);
P1 has none, and its deferral is the owner's recorded decision above.

**Update (desktop isolation, PR #158 follow-up):** the desktop previously could not drive an isolated engine. A debug-only test namespace now exists, and the desktop confirmation path has an
exact isolated validation procedure; see `DEPLOY_DESKTOP_ISOLATED_VALIDATION.md` (isolation mechanism, security review, procedure). That procedure has **not** been run; risk 4 above stays open until it is.
