# Storage visibility, Slice 1: persistent disk meter (2026-11-16)

Orchestration log. Scope approved by the owner: visibility only. Slices 2 (attribution) and 3 (cleanup) are not started; unreferenced-image removal and `all=true` BuildKit pruning are deferred.

## Plan as approved
1. One shared reading of `harpoon disk status --json`, one poll, no new engine or journal.
2. A permanently visible guest-filesystem meter on Resources and a small header chip.
3. The runtime's `pressure` is the only warning/critical classification. No thresholds are defined in the desktop.
4. Guest filesystem usage and the macOS sparse-image allocation are separate lines with their own explanations.
5. Correct the two "clean" messages so they speak about Harpoon deployment artifacts only.

## Seams
| Concern | Where |
|---|---|
| Reading, staleness, states (pure) | `src/lib/diskTelemetry.ts` (store + `diskView`) |
| Single poll, shared hook | `src/hooks/useDiskTelemetry.ts`; mounted once in `App.tsx` (an effect only, so App does not re-render per reading) |
| Meter and chip | `src/components/DiskMeter.tsx`; `ResourcesView.tsx`, header in `App.tsx` |
| Consumers moved onto the store | `DiskCapacityNotice.tsx` (its own 60 s poll removed), `DiskReclaimCard.tsx` (its own load removed) |
| Messages | `DiskReclaimCard.tsx`; assertion in `reviewOldReleases.test.mjs` |
| Unchanged | Rust/Swift runtime, `get_disk_capacity`, `noticeLevel`, expansion and reclaim flows, cleanup engine |

## States (`diskView`)
live, stale (last refresh failed, or no reading for 3 polls), engine-down (Harpoon not ready or runtime reports not running), disconnected (VM running, no guest figures), unsupported (reply not understood), unavailable (nothing read yet). A failed refresh keeps the last good reading and its time. The bar is drawn only when guest figures exist, so no state shows a zero bar.

## Attempts, failures, root causes
| # | What happened | Cause | Resolution |
|---|---|---|---|
| 1 | First `diskTelemetry` test failed (`n >= 2`) | Test subscribed after `setEnabled`, so it heard one notification | Test expectation corrected to 1; code unchanged |
| 2 | `diskMeter` test process hung past the timeout | A failing assertion skipped `unmount`, leaving the poll interval alive | Assertions wrapped in `try/finally` so the interval is always cleared |
| 3 | Shared-poll test saw 2 requests, expected 1 | The mount refresh had already completed inside `render`'s `act`; my extra explicit refresh was a second request | Test asserts after mount only, then forces the second refresh deliberately |
| 4 | Screenshot of the harness was blank | `file://` page did not load its script | Served the harness over a throwaway local HTTP server |
| 5 | `timeout` not found | macOS has no `timeout` | Used `node --test-timeout` instead |

No safety check was weakened. No replan or scope reduction was needed.

## Evidence
- Environment: Node 22.23.3, clean `node_modules` from `npm ci`.
- `npm test`: 366 tests, 366 pass, 0 fail, 0 skipped (350 before; 16 new in `diskTelemetry.test.mjs` and `diskMeter.test.mjs`).
- `npm run build` (`tsc && vite build`): passes. `npm audit`: 0 vulnerabilities.
- UI evidence: a static harness renders the real `DiskMeter` and `DiskChip` with the app's CSS in headless Chromium at 20/50/85/95% and in the stale, engine-down, disconnected and unsupported states. It used no Docker, no Harpoon and no live engine.
- Nothing in this slice calls the live engine except the existing read-only `get_disk_capacity`, unchanged.

## Known limits
- Warning/critical colour is the runtime's classification, so a desktop running against an older runtime that reports a different `pressure` shows that runtime's verdict.
- The meter does not explain what is using the space; that is Slice 2.
- "Last successful reading" is refreshed when the store changes (one poll a minute, or a touch when the window is hidden); between ticks the age text can lag by up to a minute.
