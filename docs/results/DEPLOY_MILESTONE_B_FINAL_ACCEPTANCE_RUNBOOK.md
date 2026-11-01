# Milestone B final live acceptance: runbook for L5, L6, L7, P1 and P2

Status: **prepared, not run.** Everything here needs one explicit written authorization for the live session. Preparation
(build, read-only inspection, offline tests) is complete; no Docker resource was created, changed or removed, the Harpoon VM
and process were not touched, and `Gate::shipping()` is unchanged.

## 1. Binaries (build at execution time; record every value)

| Binary | Source | Used for |
|---|---|---|
| `harpoon-deploy-acceptance` | reviewed `main` (merge of #135 or later), clean checkout, `--features acceptance` | everything except the one fault step |
| `harpoon-deploy-b15-overlay` | a **local, unpushed** commit on top of the same `main` revision | exactly one step: S5, the deterministic final-check failure |
| `harpoon-deploy` (shipping) | same checkout, no features | proof that the gate is closed |

Build (offline, clean worktree, record `git status --porcelain` empty, `rustc -V`, `cargo -V`):
`HARPOON_ACCEPTANCE_REV=$(git rev-parse HEAD) cargo build --offline --release --features acceptance --bin harpoon-deploy-acceptance`
`HARPOON_OVERLAY_REV=$(git rev-parse HEAD) cargo build --offline --release --features acceptance --bin harpoon-deploy-b15-overlay`
Then `shasum -a 256` each and `--acceptance-info` / `--overlay-info`. A rebuild at the same path and revision gave the same SHA-256
(checked in two target directories); a different checkout path embeds different paths and gives a different hash, so build once and
record that hash. The overlay adds only `--experimental-fail-final-check` to `replace` (live, `--confirm-cutover`, refused unless
the overlay binary armed it): the candidate's probes of an application port other than the serving release's pinned port report a
connection failure; the preliminary range and the restored release are real. The crash/SIGKILL overlay is **not** used (L4 covered it).
The overlay's own offline test proves a healthy candidate still fails only through the injection and is restored exactly once from its
pin; on the overlay revision the main-branch guards fail (parser, binary count, source scan), which is the intended tripwire.

## 2. Baseline and boundaries (read-only, confirmed before this plan)

Docker 28.3.3 through `/tmp/harpoon-docker.sock` (mgmt `/tmp/harpoon-mgmt.sock`). Inventory identical to the L4 final inventory:
9 exited containers (`harpoon-ui-stack-test-*`, `harpoon-ui-preview-*`, none Harpoon-labelled), 6 images, 1 volume, no listener in the
38000-38999 or 39000-39099 ranges. Harpoon runtime process `harpoon` (pid 67874 at preparation; `harpoon-deploy` is a different
program and must never be running). Re-capture all of this at S0; **any difference from the capture at S0 is a stop**.

Boundaries: the session owns only (a) the dedicated runtime directory and work directory it creates, (b) containers, images and
leases carrying the session's own application ids and deployment ids, removed only by exact id after a dry run. No prune, no force,
no VM stop/restart, no touching of any container, image or volume present at S0, the bystander excepted for read-only inspection.

## 3. Run order, expected Docker mutations, checks

Variables live in `$L/vars.sh`. **Guard first** (the L4 session overwrote L3 files because `$L` still pointed at L3): `case "$L" in
$HOME/harpoon-b-acceptance-evidence/L567-*) ;; *) exit 1;; esac` and refuse to write into a non-empty step directory.
After every step: containers/images/volumes JSON, `system/df`, Harpoon pid+etime, `ps` for `harpoon-deploy`, a copy of the journal,
its SHA-256 and `PRAGMA integrity_check`; for every HTTP probe, the response marker next to the marker in the `docker inspect` env of
the container publishing that port.

| Step | Command (binary) | Expected Docker mutations |
|---|---|---|
| S0 | read-only preflight; `doctor`; SHA-256 of each binary; shipping binary refuses all three live commands | none |
| S1 | `deploy` bystander B, `--env BY=1` (acceptance) | build 1 image; create+start B container; B lease |
| S2 | `deploy` A1, `--env EMPTY= --env EQ=a=b --env MODE=one` | image, container, lease for A1 |
| S3 L6 | `replace` A with a context whose app never answers `/health`, `--confirm-cutover`, short `--health-timeout-secs` | build 1 image; create+start then remove a preliminary container; **no stop of A1, no cutover** |
| S3b | `cleanup-image` dry run, then apply, for exactly the L6 candidate image id | remove that image |
| S4 L5 | `replace` A, healthy context, `--env MODE=two --env NEW=2 --confirm-cutover` | image; preliminary container created/removed; stop A1; create+start A2; lease |
| S5 L5 | **overlay** `replace` A, `--env MODE=three --confirm-cutover --experimental-fail-final-check` | image; preliminary create/remove; stop A2; start candidate; injected final-check failure; stop candidate; one restoration of A2 from its own pin (new container `...-restored`) |
| S6 | `reconcile` dry run, then `--apply` | remove only the failed candidate's verified container and lease |
| S7 L5 | `rollback --to <A1 release> --confirm-rollback` (acceptance) | preliminary create/remove from A1's retained image; stop restored A2; create+start A1' from A1's pin |
| S8 L7 | with Harpoon CLI idle: `mv` the dedicated runtime directory aside (rename, same filesystem); `status --docker-socket`; `mv` it back; hash; then copy, corrupt the copy, `status --runtime-dir <copy>` | **none** (read-only discovery) |
| S9 P1 | alternate direct and Harpoon builds (section 5) | per run: one image (removed by id) and, for Harpoon runs, a transient preliminary container |
| S10 P2 | `ps` evidence (section 6) | none |
| S11 | dry-run-first cleanup (section 7) | remove only session-owned resources by exact id |

Port numbers are read from each plan output, never assumed.

## 4. What each step must show

- **L5:** `docker inspect` `Config.Env` of A1 equals exactly its three variables plus its own marker; A2 equals `MODE=two`, `NEW=2`
  plus its marker (not A1's); the restored container equals A2's pin (not `MODE=three`); the rolled-back container equals A1's
  pin (not A2's); the journal copy decodes `v2` pins with the sorted names; **every container ever created has a distinct marker**
  and each HTTP `/health` marker equals the env marker of the container on that port; status after S5 shows one durable failed-cutover
  pause and S7 adds a rollback pause; no `resume` is run; restoration appears exactly once (a repeat `restore --apply` is not issued).
- **L6:** A1's container id, `StartedAt` and state before and after are identical; no `stop`/`kill` request in the Docker request
  log for A1; status shows no pause and no cutover or recovery record for the L6 deployment; the retained candidate image is listed with
  the printed next actions (`cleanup-image` by id, dry run first; no `--simulate` advice).
- **L7:** the dedicated store's journal SHA-256 before the move equals after the move back; `status --docker-socket` prints the missing-store
  warning and lists the labelled workloads read-only; container ids, states and `StartedAt` unchanged across the step; the corrupt copy
  exits 4 (fail closed) and the original is byte-identical afterwards.
- **P1:** see section 5. **P2:** see section 6.

## 5. P1 build overhead (decision recorded: no `--no-cache` exists)

Harpoon's build runs `buildx build --load --progress=plain --pull=false --label ... --iidfile` with the default cache; it has no
`--no-cache` option, so "same cache policy" means default cache on both sides. To make every build do real work, each iteration gets
its own cache-busting stamp file (same size) in an otherwise identical copy of one small Dockerfile context (`alpine:3.22`, already
local). Direct builds use the same bundled buildx binary and the same flags through the same socket. Order: D H D H ... with one
discarded warm-up of each, then N = 5 each; Harpoon runs are unconfirmed `replace` on A (builds the candidate and its preliminary
check, then stops), so no serving release changes.

Report separately, per run and as medians: (1) direct wall clock; (2) Harpoon **build step** duration from the journal (the step's
started/finished milliseconds, field triples after the release id in the snapshot record); (3) Harpoon **source preparation** step
duration; (4) the whole `replace` wall clock (includes the preliminary check; informational). **Acceptance criterion: median(2) /
median(1) <= 1.2.** (3) and (4) are reported but not part of the criterion; if the ratio is within 1.2 only when source preparation
is excluded, say so explicitly. Remove each image by exact id (dry run first).

## 6. P2 resident process

Before the session and after every command: `ps -axo pid,ppid,etime,command` filtered for `harpoon-deploy`; none may exist, and none
may appear as a child. Then watch 60 s after the last command. Harpoon's own runtime process (`harpoon`, pid recorded at S0) and the CI
runner are different programs and are recorded but not counted; its pid and elapsed time must advance consistently with no restart.

## 7. Stop conditions, cleanup, recovery limitations

**Stop and preserve evidence (no reconcile, restore, retry, removal or VM action) on any of:** Harpoon pid changed or elapsed time
reset; the bystander's id, state, `StartedAt` or health changed; any container, image or volume appears or disappears that is not in
the step's expected-mutation row; a missing ownership label on anything the session created; a second restoration attempt, or any retry,
by Harpoon; an ambiguous serving identity in `status` or two containers claiming one port; a request in the Docker log that is not in
the step's row (no prune, no force, no non-session id); `$L` guard failure; an authorization or flag refusal that was not expected;
journal integrity check failure; a command running longer than twice its printed budget.

**Cleanup:** only after the last measurement. Dry run for each of `reconcile`, `teardown` (one succeeded deployment each, never an
interrupted one) and `cleanup-image`, compare every target id with the recorded ids, then apply one at a time; remove the L7 corrupt
copy by its exact path; never delete the dedicated runtime store during the session (archive it with the evidence). Final inventory
must equal the S0 baseline exactly, except the build cache count, which is reported, not compared.

**Recovery limitations (known, accepted for this session):** a crash of the CLI mid-restoration is not exercised (L4 covered one
boundary); `restored_unverified`, `restore_failed` and `not_attempted` have no live evidence and none is provoked here; a failed
final check is obtained by the overlay because an application cannot naturally distinguish the preliminary container from the final
one (identical image, env and port inside the container); the L3 original baseline files are lost (recorded in the L4 evidence note).

## 8. Evidence layout

`~/harpoon-b-acceptance-evidence/L567-<date>/` (0700), with: `vars.sh`, `NOTES.md`, `SHA256SUMS`, `00-preflight/` (baseline JSON,
pids, ports, binary SHAs and `--*-info`, shipping refusals), `01-B/`, `02-A1/`, `03-L6/`, `04-L5-replace/`, `05-L5-fault-restore/`,
`06-reconcile/`, `07-L5-rollback/`, `08-L7/`, `09-P1/` (per-run timings CSV, medians), `10-P2/`, `11-cleanup/`, `final/` (inventory
diff against baseline), `journal/` (a copy and hash after every step). Each step directory holds `cmd.txt`, `stdout`, `stderr`,
`exit`, `before-*.json`, `after-*.json`, `docker-requests-note.txt`.

## 9. Remaining risks

The overlay is test code and its binary hash must be recorded with the evidence; P1 is sensitive to a noisy host (run nothing else);
build cache growth is expected; the forwarder listener can lag about ten seconds after teardown (wait, do not retry commands); the
guest's free memory and disk are checked by the engine's own preflight at each step, not assumed; a human decision is still needed on
the written acceptance review and on `Gate::shipping()` after the evidence exists.
