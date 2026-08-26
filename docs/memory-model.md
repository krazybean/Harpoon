# Memory Model

Memory behavior is Harpoon's defining differentiator. This document defines the memory
taxonomy, the mechanisms available for growth and reclamation, the policy engine design,
and how reclamation is observed and measured. Architecture context:
[architecture.md](architecture.md). Scope: [requirements.md](requirements.md).

## Core Thesis

The first-class optimization target is not simply low configured VM memory, but low
**resident host memory during idle and post-workload states**:

> Low post-workload resident memory, not merely low boot-time memory.

## Memory Taxonomy

Harpoon distinguishes between the following quantities. These names should be used
consistently across code, telemetry, and UI.

| Term                          | Definition                                                              |
| ----------------------------- | ----------------------------------------------------------------------- |
| Configured VM capacity        | Maximum memory the VM may grow to (`maxMemoryBytes` on the VZ config)    |
| Guest active memory           | Memory in active use by guest processes and containers                   |
| Guest cache                   | Page cache and other reclaimable kernel caches in the guest              |
| Reclaimable memory            | Guest memory that can be returned to the host without data loss          |
| Container working set         | Aggregate memory attributed to running containers                        |
| Host resident memory          | Resident memory attributed to the VM process as seen by macOS            |
| macOS memory pressure         | Kernel-level pressure signal from the host                               |

Additional policy inputs tracked over time:

- Guest available memory
- Guest active/inactive memory split
- Recent allocation rate
- Recent reclamation volume
- Host swap pressure
- Container activity (start/stop events, CPU activity)

## Mechanisms

### Growth

The VM starts small relative to configured capacity and grows into available host
capacity under load. Growth follows workload demand; Harpoon does not pre-allocate the
full configuration at boot.

### Reclamation — Spike 5 qualification

Reclamation uses Apple's supported memory ballooning primitives (`VZVirtioTraditionalMemoryBalloonDeviceConfiguration` `virtio_balloon.ko`) assisted by guest cooperation. **Spike 5 2026-08-26 qualified:** guest `MemAvailable` reclaimed `815→555 MiB` and restored `→806 MiB` `BALLOON_GUEST_RECLAIM PASS`, but host `VM XPC` `phys_footprint` did NOT shrink `507→604→627 MB` after `768` target — `BALLOON_HOST_FOOTPRINT_REDUCTION NOT OBSERVED` on tested `macOS 26` `Virtualization 1112.1.16` host. Do not market balloon as proven host reclamation until platform pressured re-measured.

Candidate steps (ordering NOT established):

1. **Balloon inflation.** The host inflates the virtio balloon by the amount of
   verified-free guest memory, returning those pages to macOS.
2. **Verification before reclaim.** The balloon target must never exceed what the guest
   reports as safely available; OOM inside the guest is a correctness failure, not an
   acceptable cost of aggression.

#### Challenge: explicit cache dropping

Prior drafts specified `drop guest page cache → balloon` as a fixed ordering.

**Downgraded to EXPERIMENTAL.** Linux page cache is valuable; destroying it without
evidence trades guest performance for a host number. Whether `echo 3 > /proc/sys/vm/drop_caches`
is necessary, beneficial, or harmful on this stack is unknown:

- It may be unnecessary if the balloon driver already handles reclaimable pages correctly.
- It may be harmful (thrashing, rebuild cost) for development workloads (Node trees, Git).
- It may be required in some configurations to make balloon inflation observable on the host.

Default expectation: **do not drop cache in normal operation** until an experiment proves it helps
host-resident reduction without regressing workload latency.

**Spike 5 result (2026-08-26):** guest reclaim `815→555→806 MiB` PASS but host `phys_footprint` `507→604→627 MB` continuous `PHYS_FOOTPRINT` from balloon-issued time did NOT drop — reclamation success is host decrease not guest free, so this is `NOT OBSERVED` host reclamation. Recorded `docs/results/SPIKE5.md` (not `docs/decisions/` pending). Pending balloon+cache-drop `drop_caches` three-way still open.

## Policy Engine

The memory controller runs inside `harpoond`. It periodically samples the signals above
and adjusts the balloon target.

### Constraints

- **No pathological oscillation.** The engine must include hysteresis (e.g., distinct
  grow/reclaim thresholds with a dead zone between them), rate limiting on target
  changes, and a cooldown after each adjustment.
- **Safety first.** Never inflate the balloon beyond guest-reported safe availability;
  respect minimum free headroom inside the guest.
- **Observable.** Every decision (sample, threshold evaluation, action) is logged and
  exposed via `harpoon memory`.

### User-selectable policies

Conceptual profiles; they do not need to exist in v0.1 unless the underlying mechanism
is stable:

| Profile      | Behavior                                                    |
| ------------ | ----------------------------------------------------------- |
| Performance  | Grow eagerly, reclaim slowly and conservatively             |
| Balanced     | Default behavior with moderate hysteresis                   |
| Aggressive   | Reclaim quickly toward minimum resident footprint when idle |

## Observability

Reclamation must be measurable. Required surfaces:

- **`harpoon memory`**: current configured capacity, host resident memory, guest
  active/available/cache, recent reclaim events and volumes.
- **Event log**: timestamped record of policy decisions (sample → decision → action →
  result) sufficient to reconstruct any reclamation after the fact.
- **Metrics over time** (GUI Memory view): macOS memory pressure, VM resident memory,
  guest active memory, guest available memory, reclaimable cache, container aggregate
  working set, and reclaimed memory over time.

Measurement notes:

- Host resident memory of the VM process is measured from macOS (e.g., `phys_footprint`
  semantics), not inferred from guest-side numbers.
- Guest figures come from the guest agent, which reads guest kernel memory statistics.

## Benchmarks Before Optimization — Spike 5 qualification (see docs/results/SPIKE5.md)

Spike 5 2026-08-26 baseline SAME-DAY same-Mac `VM XPC phys_footprint` `footprint -p` Mac 11 vCPU `footprint` stable `+5s` window (`+30s/+60s` NO natural high-water reclamation `919 MB` retained) vs Docker Desktop `4.49.0` `28.3.3` `11 vCPU ~8 GiB`: Harpoon idle `~386 MB` (representative `386→919` fresh) vs Desktop idle `954 MB` `2.5x`, workload Harpoon `~919 MB` vs Desktop `~1757 MB` (`462+161+83 564 1110`→`770+404+583 1757`) `1.9x` `nginx+Redis+Postgres 467M each` equivalent `VirtioFS` `+18 MB` comparison noise noted with `--dns 1.1.1.1 8.8.8.8` compat, demand-backed `386→919 MB` growth not preallocated — measurement qualified prominent caveat `2 vCPU vs 11 1024 vs 8 GiB` NOT normalized no universal guarantees — `ponytail: direct win, scaling and cold-start latent` — pending not yet baselined until stable mechanism:

- Idle resident memory
- Post-workload resident memory
- VM startup time
- Bind-mount filesystem performance
- Container startup latency
- CPU overhead
- Network throughput and latency
- Docker API compatibility

## Acceptance Test

See the [Memory Acceptance Test in requirements.md](requirements.md#memory-acceptance-test):
a reproducible workflow demonstrating that host-resident VM memory rises materially
under workload and falls materially after the workload stops and the policy engine runs.

## Open Questions

- Balloon responsiveness under Virtualization.framework: guest `815→555→806` reclaim latency seconds but host `507→604→627 MB` NO drop qualified — see `docs/results/SPIKE5.md`.
- Whether guest cache dropping alone achieves meaningful host-side residency reduction,
  or whether balloon inflation is always required.
- Threshold values and cooldown durations for each profile — set after benchmarking.

