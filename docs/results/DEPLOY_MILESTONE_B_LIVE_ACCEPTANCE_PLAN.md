# Milestone B live acceptance — preparation and plan

Status: **PLAN ONLY.** Written from read-only inspection of the code and the existing B7/B8.3/B10/B11/B12
records on `main` (e5881df). **Nothing was executed**: no live deployment, replacement, recovery,
rollback or cleanup, no Docker or VM contact, no build of an experimental binary. The shipping
gate (`Gate::shipping()`) is unchanged and stays closed. Each live scenario below needs its own
explicit authorization; this document grants none.

## 1. Read-only preflight: what the implementation can and cannot do today

### 1.1 Can the current code exercise live replacement and rollback?

**No. Experimental wiring is required (blocker B1).** Inspected facts:

- `replace`, `rollback` and `restore` have **no live path at all**. Without `--simulate` they refuse
  before reading anything; with it they construct a `FakeBackend`. They also do not receive the
  `Gate`, so even the B7-style gate-opening binary (`experiment/b7-live-gate`, local only, which
  opens `live_deploy_enabled` for `deploy` only) cannot reach Docker through them.
- `DockerConfig.enable_replacement` defaults to false and nothing in the CLI sets it;
  `DockerBackend::with_rollback` is reachable only from library tests.
- `deploy` live works (B7) and pins each release, but refuses a second release of an application
  (`existing_release_blocker`), so every second release must go through `replace`.

Smallest necessary change (**reported, not made; separate PR/branch needed, not on `main`**):
1. A helper extracted from `deploy`'s live branch that builds the `DockerBackend` (flags, forwarder
   check, management-channel disk probe, `Attribution::PerContainer`, `live_gates`).
2. `replace`, `rollback`, `restore`: a live branch behind a **new** `Gate` field
   (`live_replacement_enabled`, false in `shipping()`), the existing authorization phrase, explicit
   `--docker-socket/--resources-dir/--mgmt-socket/--forwarder-pid`, `Kind::Docker`, and
   `enable_replacement = true` (+ `with_rollback(target, pin)` for `rollback`). Roughly 150–250 lines.
3. Only the local, unmerged experimental binary (same pattern as B7, new name and new local branch)
   sets that field. `Gate::shipping()` and `main.rs` are untouched; the CI source guards are unaffected.

### 1.2 Other blockers and constraints found

| # | Finding | Effect |
|---|---|---|
| B1 | No live path for `replace`/`rollback`/`restore` (above). | Blocks L1, L2, L3. |
| B2 | **No deterministic real failure after cutover.** The candidate's preliminary check (test port) and final check (application port) probe the same path with the same logic, from a fresh container each time, so a fixture cannot reliably pass one and fail the other. | L2 needs an injected verdict: in the experimental binary only, a flag (e.g. `--experimental-fail-final-check`) wraps the HTTP transport so probes of the *candidate's application port* report 503. Docker, the stop, the restoration container, its pinned image/limits and its real health check stay real. L2 therefore proves the restoration path, **not** a naturally failing application. Reported separately; alternative is to accept only the preliminary-failure case. |
| B3 | Live `restore` is unwired, so an interrupted live cutover has **no tool** to bring the application back, and the kill-9 case (criterion 6) cannot be run. | Wire `restore` live with B1 (it is the abort path). Without it, the only recovery is a manual `docker start` of the previous container by exact id after label verification, which needs separate authorization at that moment. |
| — | Confirmation is the up-front flag `--confirm-cutover` / `--confirm-rollback`; the plan prints before the run, not after the preliminary check. | Use two runs: unconfirmed (prepares and checks, stops nothing, shows plan) then confirmed (rebuilds — the prepared candidate is not handed over, B10 — and re-runs its preliminary check **before** stopping anything). The unconfirmed run leaves a cancelled deployment and a retained image to clean up. |
| — | A failed cutover leaves its candidate container flagged by `unresolved()` ("should have been cleaned"), which blocks every later `deploy`/`replace`/`rollback` until `reconcile --apply` removes it. | After L2, capture evidence first, then `reconcile` (dry run, then apply) before any further scenario. Removes only recorded, ownership-verified objects. |
| — | `status` has no Docker access; no CLI flag sets runtime variables; failure output prints the failing step's log tail but no "retry" next-action text. | Acceptance criteria 7, 1 (runtime variables) and 3 (listed actions) are **not coverable** by these scenarios — see §6. |
| — | The pause after a successful rollback or failed cutover has no `resume`. | A used application stays "paused" forever in its journal: use a **fresh application per scenario**; never reuse. |
| — | B11 limits: pin-timing window after a collision-moved port; blocker detection reads one listing and ignores host listeners / forwarder lag. | Do not make results untrustworthy for L1–L3 if the stop conditions in §5 are enforced: abort if `status` shows a pinned port that is not the leased one, and record a restoration whose check is slowed by forwarder lag as a finding, not as a pass. |
| — | `cleanup-image` can remove a retained image and make a release stop being a rollback target. | Not run until L3 has finished. |

None of the B11/B12 limitations makes L1–L3 untrustworthy by itself. B1 blocks all three; B2 limits what L2 proves; B3 blocks the kill/abort scenarios.

### 1.3 Existing tools that can record the required evidence

| Needed | Existing tool (read-only unless noted) | Gap and how it is closed |
|---|---|---|
| Containers, images, volumes, published mappings, host listeners 38000–39099 | `harpoon-deploy inventory --docker-socket S --lsof-path /usr/sbin/lsof --to FILE` | Shows `harpoon=yes/no` only, not the three labels or image ids per container → add **GET-only** Docker API calls with `curl --unix-socket S` (`/containers/json?all=1`, `/containers/ID/json`, `/images/ID/json`, `/volumes`), as B7 Phase 2A did. |
| Serving release and container identity, pins' effect, pause, recovery, interruption | `status --runtime-dir RT` (`serving:`, `recovery:`, `interruption:`, pause line, `rollback to`) | None. |
| Port leases | `status` (outstanding leases) and the ledger in the journal | None. |
| SQLite deployment records | `export-journal --runtime-dir RT --to FILE` (byte copy of `deployments.sqlite3`; default rollback-journal mode, so a copy at rest is complete); `sqlite3 -readonly FILE` for queries | None; copy only while no `harpoon-deploy` writer is running. |
| Health-check evidence | the command's stdout (steps, failure, `health evidence:` notes) and the copied journal's snapshots | None; save every stdout and exit code. |
| Measured interruption, recovery outcome and duration | engine record shown by `status` and the command output | **External cross-check** needed (below). |
| Forwarder delay | none | Two background `curl` loops with millisecond timestamps (below) plus Docker `State.StartedAt`; the Harpoon runtime log used in B7 is a supplement. Plain shell, no new framework. |
| Exact before/after inventory | `inventory` + `diff` | Baseline from B7/B8.3: 9 exited containers, 6 images, 1 volume, no mappings, no listeners — **re-capture; do not assume**. |
| Consistency (SQLite vs Docker) | `reconcile --docker-socket S` **dry run** (no `--apply`), `doctor`, `plan`, `teardown`/`cleanup-image` dry runs | None. |

`tools/stage-*.sh` and `regression-*.sh` can start or stop the VM; **not used**.

## 2. Experimental build configuration (to be prepared only after approval)

- Branch: new local-only branch off `main` (e.g. `experiment/milestone-b-live`), **never pushed or merged**, containing the §1.1 wiring and the B2 fault flag, plus a binary `harpoon-deploy-b-experiment` that sets `live_deploy_enabled` and `live_replacement_enabled` and prints a loud banner and its build revision (same pattern as `experiment_main.rs`).
- Build: `cargo build --release` in `deploy-cli`; record `shasum -a 256` and `--info` revision before use, exactly as B7.
- Shipping binary (`harpoon-deploy` from `main`): all read-only commands, `teardown`, `reconcile`, `cleanup-image`.
- Required runtime values (operator supplies; none are guessed): `S` (Harpoon Docker socket), `MGMT` (management socket), `PID` (forwarder pid, process name `harpoon`), `RES` (resources dir with the bundled Buildx), `RT` (a **new** runtime dir per scenario, outside the repo), `WORK` (scratch for fixtures), `EVID` (evidence dir outside the repo and every runtime dir, mode 0700).
- Authorization flag on every mutating live command: `--authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE`.
- Fixtures (copied outside the repo): `cp -R deploy-docker/fixtures/http-marker $WORK/v1`; `cp -R … $WORK/v2 && echo '# v2' >> $WORK/v2/srv.sh` (different image id, same behaviour). Health path `/health`, container port 8080.

## 3. Common procedure (every scenario)

Variables: `X` = experimental binary, `H` = shipping binary, `DOCKER_GET() { curl -s --unix-socket "$S" "http://d$1"; }` (**GET only**).

**Pre-scenario checklist (all must hold, else do not start):**
1. `shasum` of both binaries recorded; `$X --info` revision recorded; `git status` clean on `main`.
2. `H inventory …` → `$EVID/Ln/before.txt`; plus `DOCKER_GET /containers/json?all=1`, `/images/json`, `/volumes` → `before-*.json`. Compare with the previous scenario's *after* (must be byte-identical to baseline).
3. Test ports free: `before.txt` shows no `docker-port-mapping` and no `host-listener` in 38000–39099; the forwarder pid owns the Docker socket (`lsof`).
4. `H doctor` and `H plan --runtime-dir RT --project-dir $WORK/v1 …` exit 0, "blockers: none".
5. The expected mutations (per scenario below) are written to `$EVID/Ln/expected.txt` **before** running.
6. Cleanup identities are filled in as each id is produced (deployment ids, image ids, container ids from the command output and the journal), never from names.

**After every command:** save stdout/exit code; `H status` → file; `H inventory` → file; `diff` against the previous inventory must equal exactly the expected lines for that command. Any extra or missing line, any change to an unlabelled/other resource: **STOP** (§5).

**Outage probe (L1, L2, L3, started before the `replace`/`rollback`, killed after):**
```
probe() { while :; do t=$(perl -MTime::HiRes=time -e 'printf "%d",time*1000'); c=$(curl -s -o /dev/null -m 1 -w '%{http_code}' "http://127.0.0.1:$1/health"); echo "$t $c"; sleep 0.05; done; }
probe <old-port> > $EVID/Ln/probe-old.txt & P1=$!   # port from `status`/pin, e.g. 38000
probe <new-port> > $EVID/Ln/probe-new.txt & P2=$!   # the plan's new port; if unknown, start after the unconfirmed run prints it
```
Read-only HTTP on loopback only.

## 4. Scenarios

Each starts from an empty runtime dir `RT` and a **fresh application id**, and ends with cleanup by exact identity and an inventory byte-identical to baseline.

### L1 — healthy replacement  *(needs authorization L1)*
Setup (mutations: +image I1, +container hp-D1 running, mapping+listener 38000; preliminary container and test port 39000 created then removed):
1. `X deploy --runtime-dir RT --project-dir $WORK/v1 --container-port 8080 --health-path /health --docker-socket S --resources-dir RES --mgmt-socket MGMT --forwarder-pid PID --lsof-path /usr/sbin/lsof --authorize…` → exit 0. Record D1, app id, `status` (pin present), `GET /health` marker.
2. **Unconfirmed replace:** `X replace --app APP --project-dir $WORK/v2 … ` (no `--confirm-cutover`) → prints plan + anticipated interruption; builds I2, preliminary passes; exits 0 "candidate prepared, current release untouched". Inventory diff: +image I2 only (plus preliminary container/port created and removed); hp-D1 still running, probe-old uninterrupted. *Pass:* plan printed first; current untouched; cancelled deployment recorded.
3. Start probes. **Confirmed replace:** same command with `--confirm-cutover` → exit 0.
Expected mutations: +image I3 (rebuild), +container hp-D3 running on the planned new port (not 38000), hp-D1 **stopped, kept**, mapping/listener 38000 removed, new port added, preliminary container/port removed; nothing else.
*Pass criteria:* preliminary evidence is recorded before the stop (step order in output; hp-D1 `FinishedAt` after the preliminary step's end in the journal); `status` shows `serving:` = new release, container hp-D3, `interruption: N ms`; final check evidence real (`simulated=false`), marker matches that container's own `HARPOON_DEPLOYMENT_MARKER` (`DOCKER_GET /containers/ID/json`); pin in the journal = actual image id and port; probes: outage window (last 200 on old port → first 200 on new port) **≤ recorded interruption + 1 s**; **forwarder delay** = first 200 on the new port − new container's `State.StartedAt`; `reconcile` dry run reports nothing to do.
4. **Restart survival:** export journal, then run `H status` again in a new process (and `kill -9` is *not* used here). Records identical; `doctor` exit 0.
5. Cleanup: `H teardown --deployment D3` and `--deployment D1` (dry run → review exact ids → `--apply`), `cleanup-image` for the cancelled step-2 deployment's retained image I2 (dry run, then apply), then inventory diff vs baseline must be empty.
Optional L1c (criterion 7, separate authorization, no Docker mutation): before cleanup, rename `RT` aside, run `inventory`, confirm hp-D3 still serves and is reported `harpoon=yes`, rename back.

### L2 — failed replacement and recovery  *(needs authorization L2; requires B2 flag)*
Setup as L1 step 1, plus a **bystander**: a second application deployed by `deploy` (own runtime-dir entry, own port) to prove unrelated resources are untouched.
1. Unconfirmed run as in L1 (plan, preliminary pass). Then probes.
2. `X replace … --confirm-cutover --experimental-fail-final-check`.
Expected: preliminary passes (real); hp-D1 stopped; the failed candidate started on the new port and its final check reports the injected 503; **exactly one** restoration: new container `hp-D2-restored` created from **I1's image id** with D1's pinned limits and health path, on D1's original port, labels of D2 plus `io.harpoon.restores=D1`; its own health check real; candidate (the confirmed run's deployment) stopped and **kept**; I1 and both candidate images (the unconfirmed run's and the failed one's) kept.
*Pass criteria:* exit non-zero with `recovery: restored … ms (budget 60000 ms)`; restoration duration ≤ 60 s; one `RestorePrevious` only (journal shows one recovery record; Docker shows one `-restored` container); `inspect` of the restored container: `Image` = D1's pinned id (compare journal pin), `Memory/NanoCpus/PidsLimit` equal to the pin, host port = D1's; `status`: `serving:` = D1's release in `hp-D2-restored` "(restored by D2)", pause line with `recovery restored`, failure evidence (step, kind, message), measured interruption; marker on the restored port equals that container's own marker; bystander application and all baseline resources unchanged (inventory diff shows only expected lines); SQLite copy replays and shows failure + recovery durable; restart `status` identical.
3. Capture failure evidence (logs of hp-D2 via `DOCKER_GET /containers/ID/logs` GET; `inspect`) **before** cleanup.
4. Cleanup: `H reconcile --apply` (dry run first; removes only the recorded failed candidate and its lease), `H teardown --deployment D1` (handles the original and the restored container), teardown the bystander, `cleanup-image` for I2, inventory empty diff.

### L3 — explicit manual rollback  *(needs authorization L3)*
Setup: `deploy` v1 (R1), confirmed `replace` v2 (R2, as L1), both succeeded and retained, no pause.
1. `H releases --app APP` → R2 SERVING, R1 `rollback-target`; save. `X rollback --app APP --to <R1 id> …` **unconfirmed**: output must show the data/schema warning "Application rollback only. Data, volumes and database schema are NOT rolled back.", serving vs target, interruption bound; exits 0, nothing stopped. Also run once with a wrong id and with a tag-like name: refused, journal bytes unchanged.
2. Start probes; `X rollback … --confirm-rollback`.
Expected mutations: **no new image** (image list unchanged; no buildx invocation), +preliminary container/port (removed), hp-R2 stopped, +container `hp-D3` running **from R1's image id** with R1's pinned limits and health path on a fresh port; nothing else.
*Pass criteria:* exit 0 "rolled back … verified by its final health check"; `status`: `serving:` = R1's release in hp-D3 "(rolled back by D3)", `rollback to release R1, replacing R2`, trigger-pause line naming the rollback; `inspect` hp-D3 `Image` = R1's id, labels of D3; retained history intact (R1 and R2 images and stopped containers present, none removed); SQLite copy shows `rollback_to`, `replaces`, pin, interruption, final health evidence; restart `status` identical; `releases` now lists R2 as "not earlier" (not a target). Interruption cross-check as L1.
3. Cleanup: `H teardown --deployment R1's dep` (includes D3's container and lease), teardown R2's dep, then the cancelled-candidate image cleanup; inventory empty diff.

Additional live scenarios **not authorized and not in this plan's first pass**: L1b (preliminary failure of v2 with a running release), L4 (kill -9 during cutover, then `recover`/`status`/explicit `restore` once — needs B3), restoration `restore_failed`/`not_attempted` cases, second replacement after a restoration, teardown of a restored release (B11 evidence items 3–5).

## 5. Safety rules, stop conditions, abort and recovery

**Never:** prune; force-remove or remove anything not in the journal with matching id+name+three labels; stop/restart the VM or Harpoon; change `Gate::shipping()`; touch unrelated containers, images, volumes, networks; infer ownership from a name; run `cleanup-image` before L3 ends; use any docker command except the CLI and GET-only API reads.

**STOP immediately (no further command except read-only capture) if:** an unlabelled/unrelated resource differs in any inventory diff; hp-D1/previous release is stopped before the preliminary step has passed (journal order or `FinishedAt` check); the serving release cannot be identified from `status` + `inspect` agreeing; a port or ownership conflict appears that `restore_blockers`/`reconcile` dry run cannot attribute; `status`/SQLite and Docker disagree (`reconcile` dry run reports anything unexpected, or a pin's port ≠ the leased port); a restoration exceeds its budget (60 s) or a second restoration/rollback/`RestorePrevious` appears; cleanup cannot account for every id created in this scenario.

**Abort/recovery of an interrupted test:** (1) do not re-run the command. (2) `H recover` (marks interrupted; no Docker). (3) `H status` and `H inventory` + GET captures: if the status says "application may be down", the one explicit recovery is `X restore --deployment ID --apply` (needs B3 wiring; first a dry run, which does not spend the attempt, and blockers are reported without spending it). (4) Without B3, stop and ask: the only alternative is a manual `docker start <exact id>` of the verified previous container, which needs fresh authorization. (5) Then `reconcile` dry run → review → apply, and teardown by exact deployment id.

**Cleanup by exact identity:** only via `teardown --deployment`, `reconcile`, `cleanup-image` — each dry run first, every proposed target compared with the ids in `expected.txt`, then `--apply` once with no retry. Final proof: inventory equal to baseline.

## 6. Acceptance matrix (Milestone B criteria 1–7)

Legend: **O** offline test; **B7/B8** previous live evidence; **NEW** needs new live evidence; **GAP** not covered / blocked.

| # | Criterion | Offline (O) | Previous live | Needs new live | Not covered / blocked |
|---|---|---|---|---|---|
| 1 | Dockerfile dir builds, starts with non-secret runtime variables, preliminary check **before** the previous release is replaced | O: B10 order tests (fake and fake-Docker) | B7: build, start, preliminary on a first deployment | NEW L1: the preliminary check precedes the stop in a real replacement | **GAP:** no CLI way to set non-secret runtime variables; nothing live or offline exercises them end to end |
| 2 | Interruption reported beforehand and measured | O: plan lines, recorded duration (simulated clock) | none | NEW L1 (and L2/L3 cross-checks) | — |
| 3 | Preliminary failure: nothing stopped, previous keeps running, failed release retained, actions listed | O: B10 | B8.3 L2: preliminary failure on a **first** deployment (nothing running to protect) | NEW (L1b, not in first pass): failing v2 with a running release | **GAP:** output lists the failing step's log tail but no "retry"/next-action text |
| 4 | Cutover failure: one bounded restoration, outcome and durations recorded, failed release retained | O: B11/#127 incl. crash sweeps | none | NEW L2 (restored, injected final-check failure) | `restored_unverified`, `restore_failed`, `not_attempted` live cases; naturally failing app (B2) |
| 5 | `rollback` restarts a named earlier release with its pinned config and states data/schema not rolled back | O: B12 | none | NEW L3 | — |
| 6 | History/logs survive CLI restart and `kill -9`; interrupted never `running`; incomplete cutover offers the explicit recovery | O: crash sweeps, restart tests | B8.3 L3: interrupted deployment recovered live (non-cutover) | NEW: restart survival in L1–L3; kill-9 during cutover = L4 | **Blocked** on B3 (live `restore`) |
| 7 | Deleting the store does not affect running workloads; `status` still reports labelled Docker resources | O: none specific | none | optional L1c checks workloads survive and `inventory` reports labels | **GAP:** `status` has no Docker access, so the "status reports labelled resources" half is not implemented |

Also not covered by L1–L3: the requirements' failure tests (Docker unreachable, VM stopped mid-deploy — forbidden by the safety rules here, port in use, disk full, concurrent deploys), the Milestone B stop conditions on idle cost (30 MiB RSS / 0.5 % CPU) and build overhead (≤ 1.2× a direct build), and the pause behaviour beyond what `status` shows (no `resume`, no held events). **Milestone B is not declared complete on L1–L3**; even all passing leaves criteria 1 (variables), 3, 6, 7 and the items above open.

## 7. Next executable step

1. You review this plan and decide on the §1.1/B2/B3 experimental change (a local-only branch, separate from `main`; I will not start it without your go-ahead).
2. If approved: implement and test it offline, build the experimental binary, report its sha256 and revision.
3. Re-run the read-only pre-scenario checklist (§3) against the real runtime, report the baseline, and ask for explicit authorization to run **L1 only**.
