# Milestone B live acceptance — results of L1, L2 and L3 (and criterion 7 follow-up)

Date: 2026-10-31. Plan: `DEPLOY_MILESTONE_B_LIVE_ACCEPTANCE_PLAN.md`. **Milestone B is not accepted.**
The shipping gate (`Gate::shipping()`) stayed closed and unchanged.

## 1. How these were run

Each scenario was separately authorized and run once, by hand, against the local Harpoon VM's real Docker
through the forwarder, using a **local-only, never-pushed experimental binary** (`harpoon-deploy-b-experiment`,
branch `experiment/milestone-b-live`, not in `main`). That binary opened live replacement, rollback and
bounded restore behind extra gate fields and added a test-only fault injection. Nothing from it is merged.
The Harpoon process and the VM were never restarted; no prune, force-remove, `kill -9` or other fault was used.

Raw evidence (inventories, journals, probe logs, inspect output) stayed on the operator's machine, outside the
repository: it contains machine-specific paths, ids and ports and is deliberately **not committed**. This
document keeps only the measured results.

## 2. Results

| | Scenario | Result | Measured |
|---|---|---|---|
| L1 | Healthy replacement | PASS | recorded interruption 1335 ms; host-observed outage 717 ms |
| L2 | Failed replacement, injected final-check failure | PASS | exactly one bounded restoration, 1525 ms (budget 60 s); bystander untouched |
| L3 | Explicit manual rollback of a completed deployment | PASS | recorded interruption 1361 ms; host-observed outage 765 ms; new container ready 358 ms after the engine's start returned |

What each proved, live:

- **L1.** Build, preliminary check (passed before the old release was stopped), stop, start, real HTTP final
  check whose marker matches the new container's own environment; the pin (image id, port, limits) and the
  interruption are durable and identical from a fresh CLI process; `reconcile` had nothing to do.
- **L2.** After the injected failure the original release was restored from its **pinned image id and
  configuration** (limits equal to the pin), with a real health check; the failed
  candidate was kept and flagged unresolved until `reconcile`; `status` showed the failure step, the
  recovery outcome and duration, and the trigger pause.
- **L3.** `releases` offered the earlier release; refusals (unknown id, `previous`, a tag-like id, the
  serving release) changed neither the journal bytes nor Docker. The unconfirmed run printed "Application
  rollback only. Data, volumes and database schema are NOT rolled back." and the bound, passed the
  preliminary check and changed nothing in Docker. The confirmed run re-served the earlier release from its
  exact retained image id (**no image was built**), after the preliminary check and only then stopping the
  serving release; `status` showed the rollback, `rollback_to`/`replaces` and the trigger pause; the
  rolled-back-from release is no longer listed as a target.

Measurement discipline: outage and ordering figures come from the host only (engine records and curl probe
loops with host millisecond timestamps). Docker's own timestamps come from the guest clock and were not
compared with host times.

## 3. Test-only fault injection — qualification

L2's failure was **injected**: `--experimental-fail-final-check` (experimental binary only) made the engine's
live probe return `ConnectFailed("INJECTED FAULT … no request was sent to the application; this is not a
response from it")` for application-range ports other than the replaced release's port. It is **not** a real
503 and not a natural application failure, and it exercised the recovery path, not the application's
failure behaviour. The restored release was probed for real and was unaffected. The flag does not exist in
the shipping binary.

## 4. Isolation and cleanup verification

- Each scenario used its own application, runtime directory, fixtures and resource names; a bystander
  application (L2) was verified untouched (same container id and `StartedAt`, still serving).
- Teardown was dry-run first, with every listed id compared with the evidence, then applied once. `cleanup-image`
  was used only after the owning deployment's resources were released.
- After every scenario the Docker inventory (containers, images, volumes, ports, listeners) was **byte-identical**
  to the pre-scenario baseline, container/image/volume counts matched, no leases remained and no probe
  processes were left.

## 5. Forwarder listener settling after teardown

After L3's teardown the first inventory showed one extra `host-listener 127.0.0.1:<port>` line although the
container and its port mapping were already gone. A re-read a few seconds later, and another 12 s after that,
were byte-identical to the baseline. This is the forwarder's synchronisation lag (up to about 10 s). Nothing
was done to hasten it. **Before/after inventory comparisons must wait for the listener to settle**; the first
read was preserved with the evidence rather than discarded.

## 6. Known CLI messaging issues (not fixed here)

1. `releases` still ends with a hint that includes `--simulate`, which is wrong for a live runtime.
2. The unconfirmed live `rollback` run still says "(simulated) rollback".
3. Failure output lists the failing step's log tail but no "retry" next-action text (criterion 3).
4. No CLI flag sets non-secret runtime variables (criterion 1).

## 7. Remaining Milestone B acceptance gaps

| # | Criterion | Status |
|---|---|---|
| 1 | Build, run, runtime variables | Build, preliminary check and start proven live; **runtime variables unproven** (no way to set them) |
| 2 | Health-gated replacement | Proven live (L1) |
| 3 | Preliminary failure with a running release; listed next actions | **Unproven**: a live preliminary failure was not run; no retry text |
| 4 | Bounded recovery outcomes | `restored` proven live with an injected fault; `restored_unverified`, `restore_failed`, `not_attempted` and a naturally failing application **unproven** |
| 5 | Manual rollback | Proven live (L3) |
| 6 | Crash/restart survival | L1–L3 show restart survival only; **`kill -9` mid-cutover and live `restore` of an interrupted cutover unproven** (L4) |
| 7 | State-loss visibility | Offline read-only discovery implemented (section 8); **not yet proven live**, no store was deleted |
| — | Requirements failure tests | Docker unreachable (live), port in use, disk full, concurrent deploys **not run**; idle-cost (30 MiB RSS / 0.5 % CPU) and build-overhead (≤ 1.2× direct build) stop conditions **not measured** |

## 8. Criterion 7 — state-loss visibility (offline implementation)

`status --docker-socket PATH` now distinguishes three cases. It is read-only (one `GET /containers/json`).

| Store | Behaviour |
|---|---|
| Healthy | Unchanged `status`. Docker is **not** consulted (a note says so if a socket was given). |
| **Missing** | Prints a `WARNING: deployment history is unavailable`, then the containers carrying `io.harpoon.managed=true` with container id, name, state, application, deployment and image id (as Docker reports them), marked UNVERIFIED and not history. Ownership comes from labels only; a missing or malformed `app`/`deployment` label prints `unknown`, never a guess from the name. Other containers are only counted. Without `--docker-socket`, nothing is contacted and a note says how to look. |
| **Corrupt/unusable** | Existing fail-closed error (exit 4, export advice). Docker is never consulted and nothing is shown as a fallback. |
| Docker unavailable (store missing) | `docker inspection FAILED … The Docker inventory is UNKNOWN, not empty`, exit 5. |

Nothing is adopted, recorded, recreated, stopped, removed or modified. Tests (fake Docker, isolated stores):
healthy, missing with resources, missing with none, corrupt, Docker unreachable and erroring, foreign/unlabelled/
name-lookalike containers, malformed identity labels, and a no-mutation check (only `GET /containers/json`; container
state, labels and the runtime directory unchanged).

**Limitations.** "Missing" cannot be told apart from "never deployed" (both are an absent store), hence the
wording. Discovery lists only containers, not images, volumes or leases, and cannot recover history, pins
or releases. Labels are Docker's, so anything that can create a container can forge them; the output is a
lead for an operator, not proof of ownership. Not run live: no store was deleted and the running VM was not touched.
