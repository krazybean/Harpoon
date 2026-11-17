# Storage Cleanup — Slice 3 (contextual, safe cleanup) orchestration log

Branch `feat/storage-cleanup-slice-3`, from `main` at `14beeaf` (Slices 1 and 2, PRs #216 and #217, merged). Local only: not pushed, no PR.

## Scope as approved

1. Storage Breakdown rows lead to the **existing** cleanup controls (focus only; no second engine, no bypassed confirmation).
2. One shared result presentation with three independent measurements.
3. An unfinished-deployment guard on `prune-build-cache --apply`.
4. **No image removal of any kind** (dangling, tagged-unreferenced or otherwise): no new `remove-image` API, no new dialog. Unknown ownership stays inspect-only. Existing journal-owned release cleanup is unchanged.

## Investigation (facts, read from the code)

- **Release cleanup (`cleanup-releases`)**: dry run by default. Protects the serving release, the deployment owning its container and every pinned image id, and refuses outright while any deployment is unfinished. At apply time it re-checks each object (image id, all three Harpoon labels, exactly one tag, no registry digest, no container using it, including containers with an unknown image) and removes with `force=false&noprune=true`, journaling the result. Applying needs the live-changes confirmation, which only the UI's dialog supplies. Skips are reported per item; guest free space before/after when the management channel answers.
- **Build cache (`prune-build-cache`)**: Docker's own prune with `all=false`; the preview sums records Docker marks not in use. Before this slice the apply path had no check of Harpoon's own journal.
- **Disk blocks**: `harpoon disk reclaim` runs `fstrim /` under the lifecycle lock and measures the Mac allocation before and after.
- **Expansion** is a separate flow and is untouched.
- `store::read_restored` already distinguishes "no journal" (`None`, both formats) from "unreadable" (`Err`); `writer_lock_status` reports a held writer in either format.
- `confirmAction` is a singleton dialog: a second call cancels the first.

## Hypotheses (not verified)

- Docker's `all=false` prune does not remove records a running build is using, so a concurrent build would at worst lose reusable cache. Not tested against a real engine.
- The guard therefore protects Harpoon's own deployments only. It says nothing about builds started by other Docker clients, and the confirmation now says so.

## What was implemented

| Piece | Where |
|---|---|
| Prune guard (apply path) | `deploy-cli/src/cleanup.rs` `prune_guard` |
| Guard report in the preview | `prune-guard: clear` / `prune-guard: blocked: <why>` line |
| Outcome model and figure lines | `ui/harpoon-desktop/src/lib/cleanupResult.ts` |
| Result component | `src/components/CleanupResult.tsx` |
| Row actions (focus only) | `src/components/StorageBreakdown.tsx`, `src/lib/cleanupTargets.ts`, `ResourcesView.tsx` |
| Existing controls wired to results, one-at-a-time guards | `src/components/DiskReclaimCard.tsx` |
| Fresh Mac-allocation measurement | `hostAllocationNow()` in `src/hooks/useDiskTelemetry.ts` |

### Guard behaviour (`prune-build-cache --apply`)
Checked inside the command, after the live-changes authorization and before Docker is asked to do anything, so no caller can skip it by skipping a preview.
- No journal file of either kind: allowed (nothing is recorded as deploying), subject to the existing safeguards.
- A journal another process holds for writing: refused.
- An unfinished deployment (any non-terminal status): refused, naming it and pointing at `recover`.
- A journal that exists but cannot be read, or whose lock state cannot be read (corrupt `journal.wal` or SQLite file): refused, fail closed. The file is not repaired.
- Legacy `journal.wal` and SQLite are read by the same code as every other command (`store::read_restored`); nothing is created, migrated or written.
- `all=false`, the confirmation requirement and the serialisation with other deploy operations are unchanged (asserted).
- A refusal exits 6; the desktop shows it as "Not carried out. Nothing was changed." with the engine's reason.

### Contextual actions
- Deployment images and Obsolete artifacts: "Go to Review old releases". Build cache: "Go to Check build cache". The disk-image line under the rows: "Go to Reclaim unused space".
- Focus goes to the existing button, or to its section when the button is disabled (the section shows why). Nothing is invoked; the person still presses the existing button and answers its existing dialog.
- Other images, Persistent volumes, Container writable layers and Other / unattributed carry an "Inspection only" note and no action. Active, stopped-referenced, volume, unknown-ownership and unattributed objects have no cleanup path from here.
- Each action's text states that the list is information, not permission, and that the engine re-checks when reviewing and again when applying.

### Result measurement semantics
Three lines, always in the same order, never merged:
- **Docker reported**: the engine's own accounting. Release cleanup reports counts only (image sizes are shared, so no byte figure exists). Cache prune reports Docker's reclaimed figure, labelled "an accounting figure, not guaranteed free disk space". Reclaim shows "Not applicable: this operation does not change Docker."
- **Guest free space**: the engine's own before/after `df` through the management channel. When it was not measurable, "Not measured." A change is stated only when both values exist.
- **Mac disk-image allocation**: a fresh disk-status read immediately before and after the operation (`hostAllocationNow`, accepted only if it completed after the call began). For the disk-blocks operation, the runtime's own before/after. Unknown stays "not measured"; "unchanged" is only shown for two real, equal measurements. The line says freed space is only returned to macOS by Reclaim unused space.
- Statuses: Completed, Nothing to do, Partly completed (engine removed fewer than reviewed), Not carried out (exit 6), Did not complete (engine answered failure), Result unknown (no answer, timeout, interruption). A request that returned or timed out is never shown as success. After any apply attempt the reviewed list is dropped, so a stale plan cannot be applied again.
- A reclaim failure after a successful Docker cleanup is a separate result: the cleanup's result stays as it was.

## Attempts, failures and root causes

| # | What happened | Root cause | Fix |
|---|---|---|---|
| 1 | Guard test "finished deployments allow the prune" failed with "changed after reaching succeeded" | My test persisted two statuses for the same deployment id (a real journal contradiction, which the guard correctly refused) | Distinct ids per helper call |
| 2 | An existing release-cleanup UI test saw 4 invokes instead of 2 | The apply now measures the Mac allocation before and after through the disk-status command, and the test recorded every invoke | The test harness records only engine operations |
| 3 | New UI test run hung (timeout) after two passes | A test assumed a stopped Harpoon has no inventory rows; the store keeps (and labels) the last inventory, so an assertion failed and left a page mounted | Corrected the test, and added an `afterEach` that unmounts leaked pages |
| 4 | "Double click runs once" and a reclaim test failed only in the full run | Cascade from the leaked page in #3 (two dialog hosts, two telemetry users) | Same fix; both pass alone and in the suite |
| 5 | Text assertions failed on the figure lines | `dt` and `dd` concatenate without a space in `textContent` | Tests match the real text |
| 6 | `cargo fmt --check` showed 14 differences in `deploy-cli` (clean on `main`) | My new code was not formatted | `cargo fmt` in that crate (it was clean, so only my code changed) |
| 7 | clippy `useless_format` | `format!("{first}")` | `.to_string()` |
| 8 | The screenshot harness produced an unterminated string | A newline escape was lost through two layers of quoting | Rebuilt the strings |

The guard tests were verified to bite: with the guard disabled, five of them fail (unfinished, legacy, unreadable, held writer, preview-then-change).

## Safety decisions

- The inventory is information, not authorization. Every destructive step stays in the existing command, which re-verifies at apply time; the UI never passes the inventory to it.
- No new destructive code path. The UI still invokes only `deploy_op` (`cleanup-releases`, `prune-build-cache`) and `reclaim_disk_confirmed`; a test scans for any other command and for `remove_image`, `remove_volume`, `remove_container`, `rmi`, `force`, `all=true`, and the Tauri source for any new remove operation.
- The breakdown, its helpers and the result model cannot call the runtime at all (a test asserts none imports `invoke`).
- One apply at a time from click through confirmation to result, in all three cards (an in-flight reference); a second click or repeated confirmation starts nothing.
- Nothing was run against the live Harpoon engine, Collectiv, juancastro.me or PostgreSQL; the Rust tests use a fake Docker server and temporary journals, the UI tests a scripted invoke.

## Explicitly deferred (owner decision)

- All individual image removal, including dangling and tagged-unreferenced images; a remove-image API; an image-removal dialog.
- Verifying Docker's non-forced image-delete behaviour on a disposable engine (not needed: the feature is excluded).
- Bulk removal, volume or stopped-container removal, `all=true` prune, automatic cleanup, an audit record for removals outside the journal.
- Re-measuring the build-cache figure when the confirmation opens (the dialog says "about"; Docker decides what is unused when it prunes).
- The application router's `route_table` still checks only for the SQLite journal (observed in Slice 2, not touched).

## Final validation

- `deploy-cli`: 289 tests pass (8 new: the prune guard, both journal formats, unreadable and held-writer states, preview-then-change, confirmation unchanged, `all=false`). `rustfmt --check` clean, no clippy output in the changed files.
- `deploy-docker`: 311 pass. `src-tauri`: 158 pass (neither changed in this slice).
- UI (Node 22.23.3): 433 of 433 pass (19 new in `storageCleanup.test.mjs`); `tsc` and `npm run build` pass; `git diff --check` clean.
- Screenshots (mocked data, scratchpad `storage-ui/`): `s3-breakdown-actions.png`, `s3-confirm.png`, `s3-results.png` (partial release cleanup and a build-cache prune with the three measurements), `s3-blocked.png` (prune unavailable during an unfinished deployment).
