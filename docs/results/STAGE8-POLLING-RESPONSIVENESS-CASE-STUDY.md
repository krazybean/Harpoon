# Stage 8 polling responsiveness: engineering case study

**Date:** 2026-10-16 (America/Chicago)
**Scope:** Harpoon desktop Overview/CSSH responsiveness investigation
**Status:** Archived investigation; Rust / separate is the accepted product default (2026-10-16 America/Chicago).

This document preserves the investigation as a process, not just the conclusion
that “Rust fixed it.” It records what was observed, what was tested, what was
only hypothesized, and what still needs proof. The companion implementation
notes remain authoritative for the current isolation controls,
[`STAGE8-DOCKER-AB.md`](STAGE8-DOCKER-AB.md), and
[`STAGE8-RUST-REFRESH-WORKER.md`](STAGE8-RUST-REFRESH-WORKER.md).

## Executive summary

The reported symptom was intermittent CSSH/focus/scroll lag while moving
between the CSSH workspace, stack gallery, Containers view, and the surrounding
workspace. The initial diagnostic work found animation-frame gaps of roughly
1.7 seconds on a roughly 3.5-second polling cadence. That is useful correlation,
but it did not prove that JavaScript or the WebView main thread caused the
stall. Memory contention was considered, but no complete host/guest causal
measurement established it.

The strongest isolation result was simpler: turning **Overview polling OFF**
immediately removed the symptoms, and repeated A/B/A runs reproduced that
relationship. Individual switches produced qualitative, subjective
improvements, but none identified a single responsible call. JavaScript
concurrent scheduling looked worst; staggered and serial scheduling helped
partially. Skipping unchanged React writes did not meaningfully improve the
experience.

The later Rust worker moved the same scheduling boundary out
of the WebView while retaining the same CLI transport and the same four calls:
`get_status`, `get_docker_info`, `get_counts`, and `get_config`. In the user’s
observation, both **Rust / separate** and **Rust / consolidated** removed the
freeze symptoms. Consolidation was marginal at best. This is a promising
architectural result, not yet a measured performance result: workload/freshness
parity, long-run stability, errors, lifecycle behavior, and the precise
WKWebView mechanism remain unresolved.

The accepted implementation makes **Rust / separate the production scheduler
boundary**, with only a minimal JavaScript startup fallback. The development
isolation configuration and alternate schedulers were removed after repeated
macOS acceptance. Production mode does not depend on the removed isolation
configuration; this document is historical evidence, not a live configuration
guide.

## What the investigation was trying to explain

The visible failure mode was not simply “a slow command.” It appeared as a
combination of:

- CSSH terminal or workspace focus changes feeling sticky;
- scroll input arriving late, not moving, or appearing to lock during a route
  transition;
- intermittent UI pauses while the Overview/runtime refresh cycle continued;
- apparent relationships with the CSSH workspace, focus, and scroll ownership.

The diagnostic trace and telemetry were designed to classify correlations:
timer/RAF gaps, focus and visibility, scroll ownership, actual scroll movement,
PTY throughput, process CPU time, and available host pressure counters. They
were not causal proof. In particular, an RAF gap does not by itself prove a
main-thread block, a WebKit defect, a Docker problem, or VM contention.

## Chronology

### 1. Start with the symptom, not an architecture

The first working hypothesis centered on CSSH: focus switching, xterm output,
scroll listeners, animation, and PTY batching were all plausible contributors.
The surrounding Overview/runtime scheduler was also suspicious because it ran
four periodic commands on a roughly 3.5-second cycle. Memory contention was
kept in the hypothesis set because the product is a VM-backed Docker runtime,
but there was no evidence to promote it to the cause.

The initial diagnostic additions were intentionally observational. They added
bounded telemetry and opt-in deep trace rather than changing scroll behavior,
timeouts, Docker state, VM state, or guest behavior. The useful result was a
repeatable timing pattern—RAF gaps around 1.7 seconds within a roughly
3.5-second cadence—without a proven mechanism inside WKWebView.

### 2. Add narrow isolation switches

The development-only Containers isolation surface then separated the likely
owners of work. It could independently gate the Overview polling master, each
of the four calls, Containers polling, Compose grouping, motion, stack
choreography, CSSH mounting, PTY output batching, and CSSH scroll telemetry.
The switches preserved cached values and labeled them stale where appropriate;
they did not mutate Docker, VM, guest, or container state.

This stage also exposed a methodological hazard. A temporary CSSH mount toggle
caused a scroll-lock regression in the isolation UI itself. The regression was
fixed by restoring an explicit bounded layout cell for the main Containers
content and keeping the gallery/table as the sole vertical scroll surface.
That fix repaired the experiment surface; it did not prove the original lag
was solved.

### 3. The master polling result

Turning **Overview polling OFF** eliminated the symptoms immediately. Repeated
A/B/A runs—baseline, polling off, baseline—reproduced the relationship. This
was the most valuable result because it isolated the scheduler as a class of
work without pretending to identify one command or one WebView mechanism.

The result also changed the investigation: the question became “how should
periodic refresh scheduling be owned?” rather than “which CSSH visual effect do
we delete?”

### 4. Individual disable observations

The following are the user’s qualitative observations from one-at-a-time
isolation. They are not instrumented percentages, are nonadditive, and should
not be treated as an attribution model or benchmark:

| Isolation switch | Subjective improvement |
| --- | ---: |
| `get_status` | ~40% |
| `get_config` | ~30% |
| `get_docker_info` | ~10% |
| `get_counts` | ~10% |
| Containers polling | ~10% |
| Motion | ~30% |
| Choreography | ~20% |
| PTY output batching | ~10% |
| CSSH scroll telemetry | ~10% |
| Compose grouping | 0% |
| CSSH mount toggle | Broken as an isolation control during the first attempt |

These observations weakened the claim that `get_counts`, Docker information,
Compose grouping, or a single CSSH animation was the root cause. The relatively
large `get_status` and `get_config` observations suggested that command
scheduling and response delivery could matter, but they did not establish
whether the cost was command execution, Tauri invocation, serialization,
React updates, WebView scheduling, or some interaction among them.

### 5. Change scheduling while keeping transport fixed

Three JavaScript schedules were compared:

- **Concurrent:** the existing four-call cycle;
- **Staggered:** calls offset through the cycle;
- **Serial queued:** one call at a time, with missed ticks coalesced.

Concurrent was subjectively worst. Staggered and serial were partial
improvements, with the expected freshness tradeoff for serial execution. The
`skip-unchanged` experiment did not produce a meaningful improvement, which
weakened the theory that React state writes alone were the dominant cost.

This was a useful negative result: rearranging work inside the WebView helped
some, but did not remove the class of symptom. It pointed toward moving the
scheduler boundary rather than endlessly tuning the schedule.

### 6. Do not confuse transport with scheduling

The Rust Docker socket observer was explored as a separate A/B experiment.
Its intent was to compare the existing CLI-backed path with a bounded Rust
Unix-socket observer, event invalidation, reconnect, and periodic
reconciliation. The early observer result was inconclusive and subjectively
worse. It also exposed bootstrap and lifecycle defects that were later
corrected: transport toggles could rerun bootstrap or unmount the view, the
observer needed explicit teardown, and concurrent callers needed one shared
in-progress snapshot.

This experiment must not be summarized as “the socket caused the lag.” It did
not isolate scheduling from transport, did not establish parity for all
visible fields, and did not produce the required live A/B/A proof. The current
report remains a reversible, debug-only transport experiment with explicit
unmatched fields; see [`STAGE8-DOCKER-AB.md`](STAGE8-DOCKER-AB.md).

There was also ambiguity in conversational references to earlier “A” and “B”
results. In this case study, labels are explicit:

- **A (transport experiment):** existing CLI-backed control;
- **B (transport experiment):** direct Rust Docker observer;
- **Rust / separate and Rust / consolidated:** scheduler experiments using the
  existing CLI transport.

These are different axes and must not be collapsed into one A/B claim.

### 7. Move scheduling out of the WebView

The Rust-owned refresh worker kept the transport and command implementations
unchanged. It moved the 3.5-second single-flight scheduling loop to Rust with
separate keyed publications:

- **Rust / separate:** one keyed event per completed response;

The worker honors visibility, unmount, and lifecycle replacement. The frontend
retains cached/stale shelf semantics. The consolidated worker and JavaScript
comparison schedules were removed. This does not change Docker/VM state or the
CLI transport.

The user’s direct observation was that both Rust modes removed the freeze
symptoms. Consolidation appeared marginal at best, so it was not retained.

## Evidence ledger

### Evidence observed or verified

- Initial CSSH/focus/scroll lag was observed during the investigation.
- RAF gaps of approximately 1.7 seconds on an approximately 3.5-second cycle
  were observed in diagnostic work; the exact WKWebView cause was not proven.
- Overview polling OFF immediately removed the symptoms, with repeated A/B/A
  observations reported.
- Concurrent JavaScript scheduling was subjectively worst; staggered and serial
  were partial improvements.
- Skip-unchanged updates did not meaningfully improve the symptom.
- Rust / separate and Rust / consolidated reportedly removed the freeze
  symptoms in the user’s development observation.
- The current Rust worker implementation is single-flight and has offline
  frontend/Rust checks recorded in its companion report.

### Hypotheses weakened or rejected

- A single `get_counts` call as the complete root cause.
- Compose grouping as the cause.
- Motion or choreography as the complete cause.
- Skip-unchanged React writes as the dominant cause.
- The Docker Unix socket itself as the demonstrated cause.
- A need for Redis, a second Node process, or a new message bus before first
  isolating scheduler ownership.

### Still unknown

- The precise mechanism inside WKWebView: JavaScript scheduling, rendering,
  event delivery, Tauri boundary behavior, or an interaction among them.
- Whether the Rust modes preserved identical data freshness and visible
  workload parity during the reported observation.
- CPU, memory, and command-latency costs under a representative workload.
- Behavior when commands fail, the runtime disappears, the app hides, the
  worker restarts, or the UI unmounts during a cycle.
- Whether the improvement remains stable for at least 10–15 minutes and across
  repeated application/runtime lifecycle transitions.

## Architectural decision record: candidate Rust / separate

### Decision

Treat **Rust / separate** as the default production-capable scheduler, while
keeping the current JavaScript scheduler available as the control and fallback
until live acceptance passes.

### Why this candidate

- It moves periodic scheduling out of the WebView, the seam most strongly
  implicated by the master-polling and scheduler experiments.
- It preserves the existing CLI transport and four command implementations,
  reducing parity and authority risk.
- It is simpler to reason about than consolidated publication and retains
  per-response identity and error handling.
- Consolidation showed no meaningful additional subjective benefit, so its
  extra coupling is not justified yet.

### Tradeoffs

- Rust introduces a worker lifecycle and event-delivery boundary.
- Separate events may cause more frontend event handling than a consolidated
  event, even though that cost was not shown to be material here.
- CLI-backed calls still carry their execution cost; moving the timer does not
  make those calls faster.
- The candidate can hide a freshness regression if acceptance checks only
  “feels smooth.”
- This is a scheduler decision, not a decision to replace Docker transport or
  create a second Docker state model.

### Acceptance criteria before production use

1. Capture the documented live A/B/A run on macOS/Tauri/WebView with a fixed
   dataset, route, window size, diagnostic configuration, and observation
   window.
2. Demonstrate equivalent visible data and freshness for JavaScript control,
   Rust / separate, and Rust / consolidated, including errors and stale labels.
3. Prove clean stop/restart behavior for hidden document, unmount, mode change,
   individual gates, runtime disappearance, command failure, and app shutdown.
4. Record command completion/error behavior and bounded resource impact; do not
   infer success from event counts alone.
5. Maintain at least 10–15 minutes of stable observation without freeze
   symptoms, freshness drift, duplicate workers, or unbounded event growth.
6. Add regression coverage for scheduler ownership, single-flight behavior,
   stop/wake, error propagation, freshness, and lifecycle cleanup before
   changing the production default.

## Reusable diagnostic checklist

For a future Harpoon responsiveness investigation:

1. Record the exact symptom, route, focus target, scroll surface, runtime
   dataset, window size, app visibility, and time window.
2. Establish a clean baseline and reproduce it twice before changing anything.
3. Instrument bounded observations first: timer/RAF, focus/visibility,
   scroll-owner/actual-scroll, PTY throughput, process CPU, and available host
   pressure. Label unsupported metrics unavailable.
4. Disable the broadest suspected scheduler or subsystem, then restore it with
   A/B/A. A result is weaker without the restored baseline.
5. Disable exactly one owner at a time. Verify that the switch actually stops
   its work and leaves cached data semantics honest.
6. Keep transport, workload, route, and freshness fixed while testing
   scheduling. Do not run dual observers or mix a socket experiment with a
   scheduler experiment.
7. Test concurrent, staggered, and serial only when their freshness costs are
   recorded.
8. Treat subjective percentages as directional observations: non-measured,
   nonadditive, and not causal attribution.
9. Separate user observation, code inspection, offline tests, and live
   measurement in the report.
10. Prefer the smallest ownership change that explains the strongest result.
    For this case, that means moving scheduling out of the WebView before
    considering a new socket layer, message bus, Redis, or another Node
    process.
11. End with explicit acceptance criteria and unresolved questions. “Feels
    fixed” is valuable product evidence, but it is not the same as proof of
    mechanism or production readiness.

## Related records

- [`docs/containers-ui-isolation.md`](../containers-ui-isolation.md) —
  development-only isolation controls and A/B protocol.
- [`docs/cssh-diagnostic-trace.md`](../cssh-diagnostic-trace.md) — opt-in trace
  and deep-trace procedure.
- [`docs/telemetry-cycle-1.md`](../telemetry-cycle-1.md) — bounded telemetry
  contract and interpretation limits.
- [`STAGE8-DOCKER-AB.md`](STAGE8-DOCKER-AB.md) — separate transport observer
  experiment.
- [`STAGE8-RUST-REFRESH-WORKER.md`](STAGE8-RUST-REFRESH-WORKER.md) — worker
  implementation and formal live acceptance protocol.
