# Stage 2 Resource Attribution

Captured: `2026-10-14T04:46:31Z`

Host, guest, and workload cgroup accounting are separate domains. No grand total is emitted. Docker inspect provides identity; guest cgroup files provide usage truth.

## Gate verdict semantics

- `PASS` means every Stage 2 assertion ran and passed.
- `FAIL` means the assertions ran and found a Harpoon or test defect, invalid output, or failed assertion.
- `BLOCKED_ENVIRONMENT` means the required baseline could not be established because of external or pre-existing state not owned by this test. It is non-pass and blocks dependent full-chain closure, but is not a Harpoon product failure.

State A remains strict: Docker must be reachable and the Harpoon context must contain zero containers before collection begins. If the context is unavailable because Harpoon is stopped, Stage 2 starts it, waits for bounded Docker readiness, and records that ownership; cleanup stops Harpoon only when Stage 2 started it. A failed or timed-out recovery is a truthful `FAIL`, not a stopped-runtime environment block. The gate exits `0` for `PASS`, `1` for `FAIL`, and `2` for `BLOCKED_ENVIRONMENT`. Its JSON report always includes `verdict`; no cleanup occurs for unavailable Docker or ambiguous ownership. Containers created by this harness carry the `com.krazybean.harpoon.validation=stage2` label and are the only containers eligible for automatic cleanup. A legacy container with only a matching name but no label is ambiguous and blocks the gate.

## States

| State | Meaning | Docker containers |
|---|---|---:|
| A | zero containers | 0 |
| B | one idle container | 1 |
| C | multi-container workload | 2 |

## Lifecycle

- shims: **observed**
- runc: **observed**
- samples: 13 at 0.1s

## Exit criteria

- PASS `hostProcessesAttributable`
- PASS `hostVZAssociation`
- PASS `guestInfrastructureAttributable`
- PASS `dockerToCgroupCorrelation`
- PASS `independentWorkloadTruth`
- PASS `shimLifecycleDirectlyObserved`
- PASS `runcLifecycleDirectlyObservedOrMarkedUnobserved`
- PASS `execLifecycleObserved`
- PASS `stopLifecycleObserved`
- PASS `noIncompatibleDomainsSummed`
- PASS `sameCollectorA_B_C`
- PASS `repeatable`
- PASS `machineReadable`
- PASS `humanReadable`
