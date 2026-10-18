# Containers UI isolation experiment

This is a temporary developer-only control surface for isolating recurring RAF gaps and CSSH scroll/focus stalls. It is available only on the Containers route in a development build. Settings live in React state for the current session and are not persisted.

## Toggleable effects

- **Container motion** disables Containers-scoped CSS transitions, keyframes, and loading pulses.
- **Stack choreography** removes the stack gallery enter/leave animation and its 440 ms focus delay.
- **CSSH workspace** mounts or unmounts the CSSH workspace. Unmounting tears down xterm, listeners, resize observers, diagnostic timers, and the controlled PTY session. It does not stop containers, the VM, Docker, or persistent runtime sessions. The UI warns before closing active interactive shells.
- **PTY output batching** switches CSSH terminal output between next-RAF batching and direct xterm writes.
- **Overview polling (master)** gates the steady-state 3.5-second runtime interval. When off, all four calls are skipped and their last successful values remain visible as cached data. Bootstrap/manual refresh remain available so the runtime state machine and recovery actions cannot be stranded.
- **`get_status`**, **`get_docker_info`**, **`get_counts`**, and **`get_config`** each have independent steady-state gates under the master. A disabled gate skips that command's periodic Tauri invocation/backend work; its cached shelf value remains visible. The four gates do not change route-specific Containers polling.
- **Runtime schedule** is a mutually exclusive developer-only experiment: **Concurrent** preserves the existing four-call 3.5-second batch; **Staggered** starts the four commands at 875 ms offsets inside each 3.5-second cycle, so each command keeps one nominal call per cycle; **Serial queued** runs the fixed command order one at a time and coalesces missed ticks to at most one pending cycle. Serial can make values less fresh when one cycle takes longer than 3.5 seconds. The per-shelf in-flight guard remains separate from serial's global one-at-a-time pipeline.
- **Skip unchanged updates** compares JSON-shaped responses before writing shelf data. It still records `updatedAt` and keeps errors/refresh state working, so this measures response-to-React state work rather than suppressing freshness. A small session counter is shown when writes are skipped.
- **Container polling** gates the Containers-route five-second refresh interval. Cached rows remain visible and are labeled stale; the manual Refresh button remains available.
- **Compose grouping** skips the actual Compose metadata grouping and per-stack network/volume summary transformation. With it off, containers render as standalone cards; Docker data and actions are unchanged.
- **CSSH scroll telemetry** gates CSSH wheel/scroll ownership listeners and scroll telemetry.

xterm's base renderer/theme, PTY transport itself, Docker/VM operations, and guest/container lifecycle are intentionally not toggles: they are not safely separable feature-level effects in this UI path.

## Clean baseline and trace opt-in

The frontend telemetry collector, root interaction listeners, CSSH scroll listeners, and Deep Trace are now idle unless both of these development-only switches are set before reload:

```js
localStorage.setItem("harpoon.diagnosticTrace", "1")
localStorage.setItem("harpoon.deepTrace", "1")
```

Native trace output also requires `HARPOON_DIAGNOSTIC_TRACE=1`. The normal baseline therefore does not sample host metrics, run a RAF heartbeat, write the native trace, or collect root pointer/wheel/scroll events. Diagnostics-page log polling remains scoped to the Diagnostics route only.

Unavailable or unsafe isolation points are intentionally documented rather than simulated: there is no checkbox for the xterm renderer, Docker/VM/container operations, or a fake "library off" switch that leaves its hooks and timers mounted.

## A/B protocol

1. Start from **Reset baseline / all on** and capture the all-on baseline for the same visible route and duration.
2. **B:** Disable exactly one of **`get_status`**, **`get_docker_info`**, **`get_counts`**, or **`get_config`**; keep the master on. Repeat the same reproduction and confirm the disabled command stops being invoked while its cached value remains visible and stale.
3. **A:** Reset baseline / all on and repeat the same reproduction to confirm that command resumes. Run this A/B/A sequence for each remaining command.
4. Also run one master-off check: all four periodic calls must stop regardless of the individual checkbox states; re-enable the master and confirm only selected individual calls resume.
5. Keep DevTools closed or unfocused during each run and record whether the window is focused and visible; do not mix DevTools focus with an A/B result.
6. Compare the observed behavior across runs. The experiment can support or weaken the `get_counts` hypothesis; it must not assume `get_counts` is the culprit before the one-at-a-time evidence exists.
7. Repeat the same workload with each runtime schedule. Compare call start times and UI behavior; do not treat Serial's potentially older data as an equivalent freshness baseline. Then toggle **Skip unchanged updates** independently in Concurrent mode to isolate response equality from scheduling.

The controls are investigative only. They do not claim to identify a root cause by themselves; they make the experiment boundary and teardown behavior observable.
