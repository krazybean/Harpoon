# Harpoon 2 telemetry — Cycle 1B

Cycle 1B is an observational, debug-oriented foundation for investigating CSSH
scroll lag. It does not render graphs, change scroll behavior, manage shell
timeouts, or create a second Docker/container state model.

## Contract

Each sample has `name`, `source`, `unit`, `timestampMs` (wall-clock),
`monotonicMs` (duration ordering), and `availability`. Available samples carry
`value`; unavailable or unsupported samples deliberately omit it. The frontend
ring buffer keeps at most 1,024 samples and approximately five minutes of
history. Sampling is one second; scroll and frame observations are event-driven.

Collected observations:

- Root input correlation: semantic focus in/out and active element, window
  focus/blur, throttled pointer events, captured wheel delivery, actual scroll
  events, and next-frame wheel outcomes across the master workspace, master
  Containers table, stack gallery, CSSH workspace, and xterm terminal.

- `ui.scroll.wheel.received`: wheel delta, target, nearest scroll owner, and
  directional boundary classification.
- `ui.scroll.wheel.outcome`: next-frame movement outcome retained in the
  frontend ring; native forwarding is summarized by owner.
- `ui.scroll.owner_change`: nearest scrollable owner transition, including
  xterm viewport versus surrounding workspace surfaces.
- `ui.scroll.actual`: actual scroll position retained in the frontend ring.
- `ui.scroll.latency`: native anomaly when actual scroll follows wheel input by
  more than 50 ms.
- `ui.raf.gap`: animation-frame gaps over 34 ms.
- `pty.output.throughput`: PTY output bytes per second.
- `host.process.cpu_time`: cumulative Harpoon desktop user+system CPU time.
- `host.process.max_rss`: peak process resident set size, not current RSS.

On macOS, host CPU utilization, physical memory total/free/used, memory
pressure level, pageouts, compressed pages, and swap used/available use native
sysctl counters with source and unit labels. The first CPU sample is
unavailable because utilization requires a delta. Process max RSS remains
explicitly peak RSS, never current RAM. Aggregate disk I/O is unavailable
because no stable cheap native counter was selected; VM/guest metrics remain
unavailable rather than being inferred from host values.

## Root trace volume policy

The frontend ring keeps the individual root events for forensic correlation.
The native diagnostic file is intentionally quieter: wheel, pointer, and
actual-scroll activity is accumulated by semantic owner and emitted as one
`ui.input.summary` at most every 500 ms. The summary carries event counts,
wheel delta, cumulative actual scroll delta, outcomes, and boundary state.
This bounds a burst such as 32 wheel events in 300 ms to one compact native
summary while retaining the detail in `window.__HARPOON_DIAGNOSTICS__.snapshot()`.

Individual native entries are retained for focus/window transitions, owner
changes, RAF gaps over 34 ms, scroll latency over 50 ms, errors, and confirmed
non-boundary no-scroll windows. A per-wheel `moved=0` is not an anomaly by
itself. If the same summary window has cumulative scroll movement, it is
marked `inconclusive=momentum`; boundary windows are also excluded from the
no-scroll anomaly.

Anomaly detail includes `source=frontend.root`, wall-clock `ts`, monotonic
`mono`, and the latest host CPU and memory-pressure values when available.
Unavailable host metrics are labeled `unavailable`.

## Opt-in trace correlation

The existing debug trace remains opt-in. In a development build, set
`HARPOON_DIAGNOSTIC_TRACE=1`, enable the existing `harpoon.diagnosticTrace`
local-storage switch, and reproduce the stall. The trace writer is asynchronous
and bounded; telemetry does not log every sample or every wheel event.

On macOS, Rust's `std::env::temp_dir()` resolves through `TMPDIR`, so inspect:

```sh
tail -f "${TMPDIR}harpoon-diagnostic-trace.log"
```

The optional path override remains `HARPOON_DIAGNOSTIC_TRACE_PATH`.

### Deep Trace diagnostic cycle

Deep Trace is a separate development-only layer for distinguishing WebKit
frame scheduling from a delayed JavaScript event loop. It requires both
`HARPOON_DIAGNOSTIC_TRACE=1` and `localStorage.setItem("harpoon.deepTrace", "1")`
followed by a reload. Verify the pipeline before reproducing with:

```js
window.__HARPOON_DEEP_TRACE__.snapshot()
```

The timer and RAF heartbeats are monotonic and rate-limited at the native
sink. The in-memory context ring is bounded to 256 entries and five seconds;
significant stalls retain one rate-limited five-second post window. Timer-only,
RAF-only, and both-delay classifications are diagnostic discriminants, not
causal proof. Hidden or unfocused intervals are suppressed because WebKit is
allowed to throttle them. The native heartbeat reports explicit unavailable
values when a host counter cannot be read.

## Reproduction and interpretation

With the Harpoon desktop development build running, open a CSSH stack with a
connected shell, scroll terminal history, switch between the xterm tile and
the surrounding workspace, then switch to the stack gallery and master
Containers surfaces and repeat while a command produces output. Capture
whether the stall shows:

- wheel event but no matching actual scroll delta: event ownership/xterm path;
- large `ui.scroll.latency` or `raf.gap` with low PTY throughput: frontend latency;
- large gaps coincident with rising PTY throughput or process CPU time: likely
  terminal/render or host contention, not proof of a VM cause;
- no frontend gap but delayed backend/PTY events: transport or backend path.

For the native file, inspect summary and anomaly entries first:

```sh
grep -Ei 'ui\.input\.summary|ui\.scroll\.(no_move|latency|owner_change)|ui\.focus|ui\.raf\.gap' \
  "${TMPDIR}harpoon-diagnostic-trace.log" | tail -n 150
```

These are correlations, not causal claims. Cycle 1B intentionally does not
claim VM/guest contention without a truthful VM-side source.

## Connectedness check

In a development build, `window.__HARPOON_DIAGNOSTICS__.snapshot()` proves
that the root collector mounted before reproduction. Its bounded event ring is
separate from the five-minute metric ring. Production builds do not attach the
root listeners or expose this object.
