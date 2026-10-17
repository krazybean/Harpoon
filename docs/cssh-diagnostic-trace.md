# CSSH diagnostic trace

This is a temporary development aid for diagnosing universal-command stalls. It is disabled unless both switches below are enabled, and is compiled as a no-op in production builds.

1. Start the desktop dev app with `HARPOON_DIAGNOSTIC_TRACE=1`.
2. In the devtools console, run `localStorage.setItem("harpoon.diagnosticTrace", "1")`, then reload the UI.
3. Reproduce one CSSH `Running…` stall and stop. Do not run containers or cleanup commands for this trace.
4. Inspect `"${TMPDIR}harpoon-diagnostic-trace.log"` and the optional rotated `"${TMPDIR}harpoon-diagnostic-trace.log.1"`. The logger uses Rust's `std::env::temp_dir()` on macOS, so this is normally the macOS per-user temporary directory, not `/tmp`. Set `HARPOON_DIAGNOSTIC_TRACE_PATH` to override the file path when needed. The frontend ring is in `localStorage` under `harpoon.diagnosticTrace.events`.
5. Disable with `localStorage.removeItem("harpoon.diagnosticTrace")` and restart without `HARPOON_DIAGNOSTIC_TRACE=1`.

## Deep Trace

For the CSSH↔master Containers/stack focus-switch reproduction, enable the
second development-only switch after enabling the existing trace:

```js
localStorage.setItem("harpoon.deepTrace", "1")
location.reload()
window.__HARPOON_DEEP_TRACE__.snapshot()
```

Deep Trace adds nominal 100 ms JavaScript timer and independent
`requestAnimationFrame` heartbeats, focus/blur and document visibility events,
and `PerformanceObserver` long-task entries only when `longtask` is actually
supported. Unsupported collection is reported as `deep.longtask.unavailable`.
The native side emits an asynchronous `native.heartbeat` approximately once
per second with wall and monotonic timestamps, process CPU-time deltas, and
the nearest available macOS CPU, memory-pressure, pageout, and swap values.
PTY throughput remains a frontend counter because the native heartbeat has no
authoritative terminal-output counter.

The console snapshot reports active sensors, the last timer/RAF delays, and a
bounded five-second context ring. A significant timer, RAF, long-task, or
scroll delay opens one rate-limited pre/post context window. A large RAF gap
with a normal timer is the RAF-only discriminant; large gaps in both indicate
that JavaScript scheduling was delayed too. Focus transitions and hidden
document intervals are recorded but suppressed as stall classifications.

Deep Trace is disabled unless both switches are enabled and the app is a dev
build. Disable it with `localStorage.removeItem("harpoon.deepTrace")`, reload,
and restart without `HARPOON_DIAGNOSTIC_TRACE=1` when finished.

Before reproducing, prove the frontend collector is connected in the devtools
console:

```js
window.__HARPOON_DIAGNOSTICS__.snapshot()
```

The result contains the semantic active-element classification, the bounded
root event ring, and the existing telemetry ring. A fresh result should have
`started: true`, a `ui.diagnostics.started` event, and no user-entered text,
command text, output, credentials, environment values, or container IDs.

Root events include focus transitions, window focus/blur, throttled pointer
movement/clicks, captured wheel delivery, actual scroll events, and the next
animation-frame wheel outcome. These remain granular in the bounded frontend
ring, but the native file receives at most one `ui.input.summary` per owner
per 500 ms window. Focus changes, scroll-owner changes, meaningful RAF gaps,
scroll latency over 50 ms, errors, and confirmed no-scroll windows remain
individual native events.

`moved=0` is inconclusive when cumulative scroll movement exists in the same
window; the summary labels that case `inconclusive=momentum`. A boundary is
also not a stall. Only a non-boundary window with wheel outcomes all reporting
no movement and zero cumulative scroll can produce `ui.scroll.no_move`.
Compare the semantic owner (`cssh.terminal`, `cssh.workspace`,
`master.containers`, `stack.gallery`, or `master.workspace`) across the
context switch.

Use one `trace=...` value to correlate React, Tauri, and backend events. A backend entry without a frontend settlement indicates the invoke/backend boundary is pending; continuing frontend heartbeats indicate the GUI is responsive. Missing frontend heartbeats with no backend completion is consistent with a frontend/WebView freeze. This trace never records command text, output, credentials, environment values, or container IDs.

The earlier `/tmp/harpoon-diagnostic-trace.log` instructions were incorrect for the default macOS path. Use `${TMPDIR}harpoon-diagnostic-trace.log` unless `HARPOON_DIAGNOSTIC_TRACE_PATH` is set.

Anomaly details include wall-clock and monotonic timestamps, the frontend
source, and the latest host CPU/pressure values when available; missing host
metrics are recorded as `unavailable`, never as zero. See
`docs/telemetry-cycle-1.md` for the bounded metric contract, reproduction
procedure, and truthful unavailable-metric policy.
