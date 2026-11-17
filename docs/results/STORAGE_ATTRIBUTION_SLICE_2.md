# Storage Attribution — Slice 2 (read-only) orchestration log

Branch `feat/storage-attribution-slice-2`, from `main` at `d63caa2` (Slice 1 merged). Local only: not pushed, no PR.

## Plan (approved with refinements)

Two read-only Docker `GET`s plus a read-only journal read, one new Rust module, one Tauri command, one UI section. No polling, no new destructive path.

| Piece | Where |
|---|---|
| Inventory parser, ownership rules, capped fetch, journal facts | `deploy-docker/src/storage.rs` |
| Command (one at a time, 10 s per call) | `storage_inventory` in `src-tauri/src/deploy_api.rs` |
| Strict TS validator, store, view | `src/lib/storageInventory.ts` |
| Page lifecycle | `src/hooks/useStorageInventory.ts`, `ResourcesView.tsx` |
| UI | `src/components/StorageBreakdown.tsx`, `primitives.css` |

## Investigation findings

- `DockerApi` already had `list_images/volumes/containers` and `build_cache_usage`; `deploy_metrics` is the pattern for a read-only desktop command. The desktop's existing `list_*` commands shell out to the `docker` CLI and were not reused.
- `build_cache_usage` turns missing or malformed fields into `0` (`unwrap_or(0)`), which violates "never silently zero". It was left alone (the cleanup uses it) and a separate `Option`-based parser was written.
- `/system/df` already carries images (with `SharedSize`), container `SizeRw`, volumes (`UsageData.Size`, `-1` = unmeasured) and `BuildCache`, so `/images/json` and `/volumes` were not needed. `/containers/json?all=true` adds state, mounts and `ImageID`.
- The journal ledger records ids for images and containers only. Volumes are never recorded, so a volume's ownership can only be a label claim or unknown.
- `serving_release` (used by cleanup) decides what serves now; the inventory reuses it unchanged, so "active" means exactly what cleanup protects.
- The `/system/df` call measures every volume. It is read-only but can be slow with large volumes, hence no polling.
- No disposable isolated Docker 28.3.3 engine was available, so every response shape comes from the Engine API documentation, not a live capture.

## Ponytail and UI/UX Pro Max

- Ponytail: no new CLI subcommand, no extra endpoint, no new dependency, no cache layer, no periodic refresh, one small parser per layer. The Rust output is JSON text validated in TS, the same boundary Slice 1 uses for disk status.
- UI/UX Pro Max (accordion and meter guidance): real `<button aria-expanded aria-controls>` rows, 44 px targets, visible focus ring, no hover-only content, text chips instead of colour-only meaning, long names truncate with a `title` and wrap in details, `role="status"` for state messages, no pie or segmented bar. Bars compare rows with each other and are labelled so.

## Inventory model and fields used

`/system/df`: `LayersSize`; `Images[].{Id,RepoTags,Labels,Size,SharedSize,Containers}`; `Containers[].{Id,SizeRw,SizeRootFs}`; `Volumes[].{Name,Driver,Mountpoint,Labels,UsageData.Size}`; `BuildCache[].{ID,Type,InUse,Shared,Size,Description}`.
`/containers/json?all=true`: `Id, Names, ImageID, State, Labels, Mounts[].{Type,Name}`.
Journal (read-only, no lock): ledger entries with receipts, deployments, `serving_release`.

Output per category: `status` (ok/partial/unavailable), `count`, `bytes` (only where members do not overlap) plus `bytesComplete`, `sizeBasis` (what the number means), `figures`, `counts`, `items`. Per item: `bytes` (`null` = not reported), state, container `references`, `ownership` as `{property, evidence}` pairs, `protection`, `details`. Sources report `ok/failed/oversized/invalid` per endpoint.

## Ownership rules

- **Active deployment**: the journal records the id and it is what an application serves now (cleanup's own `serving_release`).
- **Historical deployment**: the journal records the id for a release that no longer serves.
- **Harpoon labels, not confirmed**: the three labels (`managed`, `app`, `deployment`) with no journal record. A claim only. Labels that disagree with the journal are flagged.
- **Unknown ownership**: neither labels nor journal. The evidence says "Untracked does not mean orphaned." Never described as removable.
- **Used by running / stopped container**: from the container list. Unknown (`null`) when that list could not be read.
- **Persistent data**: every volume, always protected in the presentation. A volume is never classified by name; the PostgreSQL volume appears as persistent data, used by its container, with unknown ownership unless labels claim otherwise.
- An unreadable journal confirms nothing and the page says so.
- Deployment images = journal-confirmed (active or historical). Obsolete artifacts = journal-recorded, non-serving, not used by a running container. It describes; it grants no cleanup eligibility, and the existing Reclaim card stays the only cleanup path.

## Shared-layer accounting decisions

- Image sizes include shared layers, so image categories and the obsolete category carry no total ("Sizes overlap, not totalled"). Per-image `SharedSize` is shown as "N of its size is shared" or "not reported" (`-1`).
- `LayersSize` is shown once, as Docker's own accounting with shared layers counted once, not as a physical total and not as the sum of anything.
- Volumes, writable layers and build cache give a total only as "at least X (N not measured)" when some member is unmeasured. Cache figures are `null` when any record lacks a size, never a smaller sum.
- "Other / unattributed" is always "Not reliably quantifiable". Nothing subtracts the rows from guest usage, and no percentage is derived from the rows. Rows may overlap (an object can be in its own row and in the obsolete row).
- Log sizes are not measured: shown as "not measured".

## Attempts, failed tests and root causes

| # | What happened | Root cause | Fix |
|---|---|---|---|
| 1 | `deployment_images_...` failed with an unexpected `label-journal-mismatch` | My fixture labelled the serving image `dep_1` but the journal said `dep_2`; the rule was right | Fixed the fixture (and added a deliberate mismatch test) |
| 2 | Cache-missing-size test failed after scaling sizes to MiB | Test replaced a literal `Size":50` that no longer existed | Updated the literal |
| 3 | `cargo build` from the repo root: "could not find Cargo.toml" | No root workspace | Built per crate |
| 4 | UI test run hung (120 s) | Two failing tests left components mounted, so intervals kept the process alive | Fixed the tests; ran with `--test-timeout` |
| 5 | `CSS.escape` undefined in jsdom | Test helper | Selected by `[id="..."]` |
| 6 | "never says safe to remove" test matched the sentence "Unknown does not mean ... safe to remove" | Regex too broad | Narrowed to affirmative phrases |
| 7 | Inventory-failure test saw `stale`, not `unavailable` | The store is a shared singleton and an earlier test left a good inventory | Accepted either, since both state the reason |
| 8 | `JSON.stringify(1e999)` produced `null` in the rejection test | JSON cannot carry Infinity | Wrote the literal into the text |
| 9 | Cache row showed "In use" twice | Same label for protection and state | New state `build-in-use` ("Docker says a build uses it") |
| 10 | Evidence list used a larger font | Default `ul` styling | `text-body` class |

## Safety decisions

- No new destructive code. The Docker module gains `fetch`, which issues exactly two `GET`s (asserted by a test that records the request lines). `call_capped` and its enum became `pub(crate)`; nothing else in `api.rs` changed.
- A body over the 4 MiB cap is never parsed (`Document::Oversized`); an unusable endpoint makes only its dependent categories "unavailable".
- A fixture-sync test fails if the committed `storage-inventory.json` differs from what the module writes, so the TS parser is tested against real module output.
- Native concurrency is bounded in Rust (a single slot held by the work itself, refused while busy) and again in the page store (no second call while an abandoned one runs). A timed-out request cannot overwrite newer data.
- Disk telemetry is a separate store; an inventory failure leaves the meter untouched (tested).
- Nothing was run against the live Harpoon engine, Collectiv, juancastro.me or PostgreSQL.

## Deferred

Slice 3 (contextual cleanup), unreferenced-image removal, `all=true` BuildKit prune, periodic refresh, log-size measurement, killing a timed-out native request, per-layer sharing detail, verification against a real Docker 28.3.3 engine, a cap raise if real fixtures need it (`storage::MAX_BODY`), the `AGENTS.md`/roadmap update after merge.

## Final validation

- `deploy-docker`: 305 tests pass (18 new in `tests/storage_inventory.rs`, incl. journal facts, GET-only, oversize, fixture sync).
- `src-tauri`: 152 tests pass (1 new: the single-slot guard).
- UI (Node 22.23.3): 408 of 408 pass (21 new in `storageInventory.test.mjs` and `storageBreakdown.test.mjs`); `tsc` and `npm run build` pass.
- `git diff --check` clean. `cargo clippy` shows no warnings in the new code.
- Screenshots (mocked data, scratchpad `storage-ui/`): `resources-collapsed.png`, `resources-expanded.png`, `resources-narrow.png` (390 px). Stale and error states are covered by tests, not screenshots.
