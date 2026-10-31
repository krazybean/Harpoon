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

### What the new tests exercise
| Kind | Tests | What it is |
|---|---|---|
| Real `DockerBackend` + real engine against a **simulated** Docker server and a fake Buildx | `deploy-docker/tests/backend.rs`: `a_crash_at_every_write_is_recovered_by_label_with_the_real_backend` (healthy), `…_of_a_failed_preliminary_check_…`, `…_of_a_failed_final_check_…`, `…_of_a_port_collision_…`, `…_of_a_failed_removal_…` | A crash at every journal write of each scenario, then restart: the deployment is marked interrupted, a new backend reconciles by label, and the tests assert that no unrelated container or image (nor the base image) is touched, no request is forced or a prune, a ledger entry recorded as released really is gone, no entry is left "intended", and replaying every record written before and after the restart reproduces the final accounting. The failed-removal case keeps Docker refusing removals after the restart (nothing may be recorded as released that still exists), then lets Docker recover. |
| Real **CLI**, real journal files, real `DockerBackend`, **simulated** Docker server | `deploy-cli/tests/cli.rs`: `an_interrupted_deployment_is_recovered_reconciled_and_only_then_lets_the_next_one_run`, `a_reconcile_that_cannot_finish_…`, `a_refused_ownership_stays_terminal_…` | A gate-open `deploy` runs in a separate process and is killed with SIGKILL while it starts its preliminary container. Then: `deploy` refused → `recover` (Docker not contacted) → still refused → `reconcile` dry run (writes nothing) → `reconcile --apply` (only the recorded id, `force=false`, no image deleted) → `status` clean → the next deployment is permitted. The second and third tests show a reconcile that cannot finish, and a refused ownership, keep the runtime blocked. |
| Against a **real Docker Engine** | none of the above | The only real-engine evidence remains the B7 live acceptance. Failure and interrupted-recovery paths have not been run against a real engine. |

Remaining gaps after B8.1 (unchanged by it): live failure and interrupted-recovery behaviour against a real engine; the BuildKit daemon's behaviour after a build is killed at its deadline; a container that ignores the stop signal; guest private-NAT exposure; intermittent forwarder startup lag; release replacement and rollback (engine state machine only, no Docker implementation); F1 and F2 above.
