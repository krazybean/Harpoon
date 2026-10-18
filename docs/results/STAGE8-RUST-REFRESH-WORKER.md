# Stage 8 — Rust-owned refresh worker

**Status:** `ARCHIVED — Rust / separate is the product default` (2026-10-16 America/Chicago)

This stage establishes Rust / separate as the production scheduler boundary
while retaining a minimal JavaScript startup fallback. The development-only
isolation controls and alternate schedulers were removed after acceptance. It
does not change Docker/VM/guest state or the CLI transport.

## What changed

The final product keeps one Rust mode:

- **Rust / separate:** one native worker schedules the same `get_status`,
  `get_docker_info`, `get_counts`, and `get_config` implementations and emits
  one keyed Tauri event per completed response.

The worker is single-flight: one cycle runs at a time, stop wakes it, and mode
changes, unmount, hidden document state, and individual gates stop or restart
it. Shelf data remains cached with frontend-owned stale labeling.
`skipUnchangedUpdates` still suppresses unchanged React state writes while
advancing freshness timestamps.

Worker replacement is serialized and joins the prior worker before launching a
new generation. Every native publication carries that generation, and the
frontend rejects stale generations. A worker start returns an error before the
JavaScript fallback is selected; a failed start cannot leave an untracked
worker behind.

The experimental JS modes (Concurrent, Staggered, and Serial queued) and the
consolidated Rust publication were archived and removed. The worker waits 3.5
seconds before its first cycle. Changed-only native publishing was not added;
the existing shelf comparison is the smaller equivalent seam.

If the native worker command is unavailable or rejects at startup, the
frontend uses the emergency JS fallback. Worker publication is generation-
checked so a superseded worker cannot publish after a restart.

## Fixed protocol

Keep Docker transport fixed. Use the same running Harpoon runtime, Docker
dataset, visible Overview route, diagnostic/telemetry switches, window size,
and 60-second observation window for every run. Do not create, stop, remove,
prune, pull, build, or mutate containers/images/volumes/networks.

1. **Baseline:** select `Concurrent`, enable Overview polling and all four
   individual gates, observe for 60 seconds, and reproduce the lag before
   recording interaction/scroll observations.
2. **Rust separate:** select `Rust / separate`; wait one full 3.5-second cycle,
   then repeat the identical 60-second observation.
3. **Rust consolidated:** select `Rust / consolidated`; repeat the same wait
   and observation.
4. **Repeat baseline:** select `Concurrent` again and repeat the baseline to
   check for drift or learning effects.

Record mode, gate fingerprint, runtime/container dataset identity, route,
timestamps, navigation/scroll latency, and any worker event or command error.
The worker's `cycle` and publication shape are supporting evidence, not proof
that the UI stayed responsive.

## Evidence boundary

Verified offline on this checkout:

- frontend tests: `34/34` pass;
- frontend production build: pass;
- focused Rust refresh-worker tests: `2/2` pass;
- Rust compile check: pass;
- full Rust tests: `27/27` pass;
- Rust formatting check: not clean because the pre-existing dirty checkout
  contains unrelated unformatted Rust changes; no broad formatting rewrite was
  applied.

Not yet verified: macOS/Tauri/WebView live lag reproduction, event delivery
under real Docker load, measured JS-vs-Rust latency, CPU/memory impact, stable
10–15 minute behavior, and whether consolidated publication improves
responsiveness. No measured performance win is claimed until the A-B-A run is
captured.
