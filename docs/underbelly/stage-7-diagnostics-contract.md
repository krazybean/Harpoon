# Stage 7 — Diagnostics Contract

**Status:** implemented; deterministic Stage 7 barrier PASS (2026-10-14 America/Chicago). Full prior-chain rerun is currently `BLOCKED_HOST_STATE` by a pre-existing Docker container rejected by the Stage 2 zero-container baseline.

Stage 7 adds a reusable, machine-readable collector to the existing foreground
Swift runtime. It does not add `harpoond`, a guest agent, a Docker state store,
or a UI dependency.

## Surface

```text
harpoon diagnostics snapshot --json
harpoon diagnostics explain
harpoon diagnostics subscribe --count N --interval SECONDS --json
```

`snapshot` is a point-in-time document. `subscribe` is bounded polling over the
same collector, suitable for a later UI shelf. It deliberately emits snapshots,
not invented lifecycle events: the current architecture has no durable event
source independent of the lifecycle and Docker observers. A future consumer can
diff snapshots and classify changes without a new daemon.

## Authority and semantics

| Domain | Truth source | Additive with other domains |
|---|---|---|
| HOST | Harpoon status, macOS `ps`, host configuration | No |
| GUEST | management vsock, `/proc`, guest process table | No |
| WORKLOADS | Docker inspect for identity; Linux cgroup v2 files for usage | No |

Every emitted metric has `name`, `unit`, `scope`, `domain`, `source`, `status`,
and `reliability`. Unavailable values carry no `value`; they are never replaced
with zero. There is no grand total. Docker inspect is correlation metadata, not
resource truth, and `docker stats` is not used.

The current Virtualization.framework association remains explicitly unavailable:
the public runtime surface does not expose a reliable per-VM host metric. This
is a truthful limitation, not a missing zero. Host process metrics are
best-effort; guest cgroup values are authoritative when the file is present.

## Boundaries and security

The guest collector uses the existing mode-0600 management socket and preserves
argv boundaries. Its probe is fixed and bounded (at most 512 KiB of response,
bounded process/file enumeration). Docker remains the authority for identity,
lifecycle, health, and metadata. Linux cgroups remain the authority for
workload resource usage.

## Provenance

The contract is provenance-first: every value names its source and reliability,
and unavailable observations remain explicit.

## Exit evidence

- snapshot schema and example fixture parse as JSON;
- host, guest, workload, and health domains are structurally distinct;
- prior Stage 0–6 gates remain required by the Stage 7 gate;
- `diagnostics explain` warns about non-additivity and unavailable values;
- no production lifecycle or Docker behavior changed.
