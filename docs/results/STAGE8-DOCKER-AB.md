# Stage 8 Docker transport A/B proof of concept

Status: **ARCHIVED — CLI transport retained; observer experiment removed** (2026-10-16 America/Chicago)

This document preserves a completed development-only experiment. The existing
CLI-backed path remains the product path (A); the direct Unix-socket observer
(B) was not promoted and has been removed.

## Paths

- **A — control:** the existing Containers view, `list_containers`, `get_counts`,
  and `get_docker_info` commands. The existing Overview/runtime scheduler and
  five-second Containers refresh remain the comparison path; transport changes
  do not rerun bootstrap or remount the view.
- **B — observer:** a Rust-owned observer in
  `ui/harpoon-desktop/src-tauri/src/docker_observer.rs`. It performs a bounded
  Docker Engine HTTP snapshot over the Harpoon Unix socket, keeps a bounded
  in-memory snapshot, listens to `/events`, marks the cache dirty on event
  bursts, reconnects after disconnect, and periodically reconciles with a
  fresh snapshot. It is compiled and registered only for debug builds.

B returns the same row/count/version shape used by the existing Containers
view where the Engine summary supplies it. B does not perform per-container
inspect calls, so attachment details that depend on CLI inspect enrichment may
be absent. Harpoon status and config remain CLI-backed in both paths because
they are not Docker Engine resources. Host/VM metrics are not part of either
path. These are deliberate unmatched fields, not parity claims.

## Live A/B procedure

1. Build and launch the development desktop without changing the VM or Docker
   workload. Use one fixed Containers dataset and the same window size, route,
   stack/table view, scroll path, and duration for every run.
2. In **UI isolation**, reset to **baseline / all on**. Keep the direct observer
   off and record A for at least three 3.5-second runtime cycles plus one
   Containers refresh cycle. First confirm that the known lag reproduces in A;
   if it does not, record **INCOMPLETE** and do not claim a transport result.
3. Repeat A once to check the symptom is not a one-off. Keep DevTools closed or
   unfocused. If using existing diagnostic evidence, enable only the documented
   opt-in trace switches before reload.
4. Reset the same baseline, enable **Direct Docker observer (B)**, and repeat
   the exact interaction. Do not change the dataset or disable polling while
   comparing transport. The sidebar labels status/config as unmatched.
5. Compare observed freeze/scroll behavior, refresh frequency, and the existing
   bounded telemetry/trace output. A lower call count alone is not success;
   B must show equivalent visible container rows/counts and lifecycle actions.
6. Toggle B off and repeat once more as A. This A/B/A check guards against
   session state, cache warmth, and workload drift.

## Acceptance gates

- **A reproduction:** the same harness visibly demonstrates the reported lag.
- **Parity:** B displays the same dataset and preserves existing container
  navigation and lifecycle controls; unmatched fields are listed above.
- **Improvement:** only claim conditional improvement if the same interaction
  has less observable blocking with equivalent update freshness and no hidden
  suppression of user-visible changes.

Offline checks cover response bounds, chunk decoding, CLI-shape normalization,
and event-burst dirty-state coalescing. Frontend tests and production build
pass. No live Docker socket, VM, container, image, volume, or network was
contacted by this implementation run; reconnect and live parity remain pending
until the procedure is performed on the user's development runtime.
