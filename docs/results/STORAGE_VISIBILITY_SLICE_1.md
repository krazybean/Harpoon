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
- `npm test`: 366 tests, 366 pass, 0 fail, 0 skipped (350 before at the start of this slice, 384 now; the new files are `diskTelemetry`, `diskMeter`, `diskLifecycle`, `diskSafety`).
- `npm run build` (`tsc && vite build`): passes. `npm audit`: 0 vulnerabilities.
- UI evidence: a static harness renders the real `DiskMeter` and `DiskChip` with the app's CSS in headless Chromium at 20/50/85/95% and in the stale, engine-down, disconnected and unsupported states. It used no Docker, no Harpoon and no live engine.
- Nothing in this slice calls the live engine except the existing read-only `get_disk_capacity`, unchanged.

## Corrective review (owner request, same slice)
| # | Finding | Change | Regression test |
|---|---|---|---|
| 1 | A remembered `pressure` (Healthy/Warning/Critical) was still shown for stale, engine-down, disconnected, unsupported and unavailable readings | `diskView.pressure` is the runtime's class only when status is `live`, otherwise `unknown`. Historical usage (percent, used/free) stays for stale and engine-down, worded "was used at the last reading", with a neutral bar. Unsupported shows no figures. The chip's accessible name no longer claims health when stale | `diskTelemetry.test` (all states), `diskMeter.test` (no health label, neutral bar, chip) |
| 2 | "Last successful reading N ago" only updated when the store changed, so it froze if refreshes failed or hung | `useDiskView` owns a 15 s tick (`TICK_MS`) independent of any request; `DiskView.now` carries the clock. The earlier `touch()` path is removed | `diskLifecycle.test` (advances after a failure, while a request hangs, and turns stale by age with no failure) |
| 2b | Found while testing: one request that never answers kept `inflight` set forever, so no later refresh could run | `refresh` abandons a request after `FETCH_TIMEOUT_MS` (20 s) as a failure and clears the timer | `diskTelemetry.test` (hung request), `diskLifecycle.test` (recovers after the timer fires) |
| 3 | Subscription/timer lifecycle under React Strict Mode | The polling effect owns the poll timer and the `enabled` flag (cleanup clears the timer and sets `enabled` false, so nothing claims to be live when nothing polls). Concurrent refreshes share one request. The store's default clock is now `() => Date.now()` (it captured the function at creation) | `diskLifecycle.test`: one request and one poll timer after the double mount, one tick timer per subscribed view, nothing left after unmount (timers 0, subscriptions 0, enabled false); toggling enabled does not stack timers |
| 4 | Safety of the notice and reclaim on non-live readings | Notice: a stale warning is still shown ("was running low at the last reading"), so a failed refresh cannot hide it, but "Review expansion" and "Confirm expansion" require a live reading (also re-checked inside `confirm`). Reclaim: the button needs a live reading (unchanged in effect from before, now explicit) and explains why it is off. Unsupported, disconnected and engine-down show no warning, as before | `diskSafety.test` (real components): nothing read; live warning with cancel and confirm paths; failed refresh keeps the warning and disables both actions; a reading that goes stale mid-review cannot be confirmed; unsupported / disconnected / engine-down |
| 5 | Visual evidence of the whole page | The real `ResourcesView` and header chip inside Strict Mode with a mocked `invoke` (no engine, no Docker). Live: warning meter. Stale: neutral bar, "8 min ago" reached by the real tick after the clock was advanced and a refresh failed, reclaim disabled with its reason | scratchpad `disk-ui/resources-live.png`, `resources-stale.png` |

Further small fixes from the screenshots: error text no longer shows a leading "Error:", and the reclaim card's allocation line says "Current allocation appears once disk status is live" instead of implying the disk does not exist.

Failures and root causes in this pass: the elapsed-time tests initially failed because the store captured `Date.now` at creation (fixed above, which also made a real clock-injection bug visible); a later test hung because an earlier test left a never-answering request in flight, which exposed the missing timeout (2b); my "all non-live states" test mutated its own reply array while looping (test bug, corrected).

## Timeout review (final inspection before the PR)
| Guarantee | Result | Evidence |
|---|---|---|
| A late reply from a timed-out request cannot overwrite newer telemetry | **Held by construction**: only the winner of the `race` is applied, and a new request cannot start until the old call has ended, so no older reply can arrive after a newer one | `diskTelemetry.test` "a late reply from a timed-out request is discarded…"; `diskSafety.test` late live reply leaves controls disabled |
| Repeated timeouts do not cause unbounded concurrent native operations | **Did not hold; fixed.** With a call that never ends, every poll after a timeout started another `disk status` process (about one a minute, each holding a blocking thread in the desktop). The store now keeps the one runtime call in a `native` slot: while it has not ended, refreshes fail immediately with "the previous disk status request is still running" and start nothing. At most one runtime call exists | `diskTelemetry.test` "repeated timeouts never run a second native request…" (ten polls, one call; resumes when it ends). The test failed before the fix |
| Stale telemetry cannot re-enable expansion or reclaim | **Held.** The controls require status `live`; a timeout makes the status stale and the late reply is discarded, so only a refresh that really completes re-enables them | `diskSafety.test` "a timed-out refresh leaves both actions disabled…" |
| Timers are cleaned up | **Held.** The abandon timer is cleared on success, failure and timeout; poll and tick timers and subscriptions are released on unmount (Strict Mode) | `diskTelemetry.test` "the abandon timer never outlives a request"; `diskLifecycle.test` |

A second defect was found by the new test and fixed: `refresh` cleared its `inflight` marker in a `finally` inside the async body, and a body that finished without awaiting (the new "previous request still running" path) ran that `finally` before `inflight` was assigned, leaving a settled promise stuck there so every later refresh returned it without running. The marker is now cleared with `.finally()` on the returned promise, after the assignment.

Two lifecycle tests then failed by design: they let a hung call stay hung and expected the next refresh to run. They now end the runtime call first and assert that no second call starts while it is still running.

## Known limits
- **The desktop-side timeout does not cancel the underlying runtime command.** `get_disk_capacity` runs `harpoon disk status --json` as a child process on a blocking thread; abandoning the wait in the UI leaves that process running until it ends. The store now allows at most one such call at a time, so a permanently hung runtime costs one process and one thread, and disk status stays "failed/stale" (reclaim and expansion stay disabled) until that call ends. Cancelling it would need a kill-on-timeout in `main.rs`, which is outside this slice.
- Warning/critical colour is the runtime's classification, so a desktop running against an older runtime that reports a different `pressure` shows that runtime's verdict.
- The meter does not explain what is using the space; that is Slice 2.
