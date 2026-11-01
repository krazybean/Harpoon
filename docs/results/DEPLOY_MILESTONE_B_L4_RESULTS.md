# Milestone B live acceptance — L4 results: crash during cutover, explicit restoration

Date: 2026-10-31. **Result: PASS for the one scenario run.** Milestone B is not accepted. The shipping gate
(`Gate::shipping()`) stayed closed and unchanged; the experimental binary used (a local, never-pushed build with
live replace/rollback/restore wiring and test-only fault injection) is not in `main`.

## What was and was not tested

This was **one injected crash at one controlled boundary**, run once. It is not exhaustive crash testing: no other
crash point (before the stop, during the start, during a restoration, during journal writes), no natural failure, no
power loss and no concurrent writer were exercised.

The crash was an **injected test fault** (`--experimental-crash-after-stop`, experimental build only): after the
previous release's container had stopped, the deployment CLI process (only that process) killed itself with SIGKILL.
Harpoon and the Docker VM were not touched.

## Results

| Item | Result |
|---|---|
| Exit status of the crashed `replace` | **137** (SIGKILL); no stdout (the report is written at the end of a run) |
| Boundary | The deployment was saved durably (`replaces` the serving release, the candidate's pinned configuration, cutover begun) **before** `stop_current`. The wrapper confirmed from Docker, by Harpoon labels, that the previous container was stopped, and only then killed the process before `start_new` |
| Order, host clock | preliminary check finished → cutover record saved ≈ 48 ms later → application stopped answering ≈ 14 ms after that → process dead within ≈ 0.1 s |
| After the crash | previous container stopped (Docker exit 143, a normal stop); **no candidate and no preliminary container existed**; the application refused connections (HTTP 000); the bystander kept answering |
| Durable state | before `recover`: deployment recorded as `running` with an open cutover, `replaces`, the candidate pin, a reserved port lease and a retained candidate image. After a fresh-process `recover`: `interrupted`, "application may be down", Docker not contacted, inventory unchanged |
| `restore` dry run | journal and inventory byte-identical; printed the pinned image id, port and limits |
| Restoration | **exactly one** `restore --apply`; outcome `restored`; **1807 ms** (budget 60000 ms) |
| Pinned image and limits | restored container's image id equals the pinned id exactly; port equals the pin; memory 256 MiB, 0.5 CPU, 64 PIDs equal the pin; restart policy `no`; carries the three Harpoon labels plus the restoration label naming the restored deployment |
| Real health | HTTP 200 whose marker equals that container's own environment value (a new container, so a new marker) |
| Serving identity | `serving_release` named the restored container, "(restored by …)"; a fresh process reproduced it; SQLite passed `integrity_check` |
| Second restore | refused (exit 6) before any Docker request; journal and inventory unchanged; one restored container only |
| Bystander (another application) | same container, never modified; **1188 of 1188 probes HTTP 200**; Harpoon's pid and start time unchanged throughout |
| Cleanup | dry run first for every step; each target compared with the recorded ids; `reconcile` (a ledger lease), `teardown` of the two applications' successful deployments, `cleanup-image` of the candidate image; no prune, no force |
| Final inventory | after the forwarder settled (two reads 12 s apart), **byte-identical to the original baseline**; container, image and volume ids identical; no leases, nothing unresolved, no listeners or probes left |

## Downtime (host-only timestamps)

- **Total observed outage: 47,695 ms**, from the last HTTP 200 before the stop to the first HTTP 200 after the
  restoration.
- **46,791 ms of it was deliberate operator inspection** between the crash and the start of `restore --apply`
  (read-only checks, the checkpoint and the dry run). It says nothing about Harpoon's speed.
- **Restoration: 1,807 ms** by the engine (1,831 ms command wall time); the first 200 came 904 ms after the
  command started.
- The journal's recorded interruption (48,558 ms) spans the same window, including the operator delay.

## Defects found

1. Before `recover`, `status` reported the deployment as `running` and did not say the application might be down.
2. After a successful restoration, the wording gave no explicit statement that the interrupted deployment had not
   succeeded and that the service was restored.
3. The interrupted-cutover hint advertised `--simulate` on a Docker runtime and implied a shipping build could
   restore it.
4. A missing store was reported as "nothing has been deployed", which a missing store cannot establish.

These are corrected in the same change as this document (see the status tests).

## Evidence handling

Raw evidence (inventories, journal copies, probe logs, inspect output) stayed on the operator's machine, outside the
repository, because it holds machine-specific paths, ids and ports. It is not committed.

**L3 evidence overwrite.** During L4 preflight a wrongly set evidence-directory variable made three L4 baseline reads
overwrite L3's `before-containers.json`, `before-images.json` and `before-volumes.json`. **The original L3
snapshots no longer exist.** The new reads (same container, image and volume ids as L3's `final-*` files, differing
only in relative "exited N ago" text) were moved into the L4 directory, and a note was left in the L3 directory. L3's
inventory text files (`before.txt`, `final.txt`) were not touched, and L3's pass relies on those and on its other
evidence, not on the lost JSON snapshots.

## Remaining Milestone B acceptance gaps

Criterion 6 is now covered live for one `kill -9`-equivalent crash mid-cutover followed by an explicit restoration. Still open:
criterion 1 (non-secret runtime variables have no way to be set), criterion 3 (a live preliminary failure with a
running release; listed retry actions), criterion 4 (`restored_unverified`, `restore_failed`, `not_attempted` live,
and a naturally failing application), criterion 7 (live: no store has been deleted), the requirements' failure tests
(Docker unreachable, port in use, disk full, concurrent deploys) and the idle-cost and build-overhead stop conditions.
