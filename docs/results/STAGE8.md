# Stage 8 — Performance Investigation

Status: **IN PROGRESS — live APFS reclamation evidence pending the self-hosted macOS runner**

## Scope

Stage 8 starts with the unresolved M16 question: after Docker deletes data from
guest ext4, does host-visible allocation of Harpoon's sparse backing file decrease?
Guest `df` alone is not sufficient evidence.

The probe is `tools/stage-8-disk-reclamation-check.sh` and is dispatched by
`.github/workflows/stage-8-disk-reclamation.yml` on the existing
`self-hosted, macOS, harpoon` runner.

## Safety contract

- Creates one uniquely named temporary image from a generated tar archive.
- Removes only that temporary image.
- Does not remove containers, volumes, user images, or user build cache.
- Attempts guest `fstrim` after deletion and records failure as unavailable evidence.
- Does not run `docker system prune` or broad `docker builder prune`.

## Measurement

`reclamation.csv` records, for baseline, image-created, image-removed, and
after-fstrim: logical backing-file bytes, allocated host blocks and block size,
and guest root used/free bytes. A drop in guest free space without a corresponding
drop in host allocated blocks is not physical APFS reclamation.

## Pruning decision

No automatic prune feature is justified yet. Docker already owns image and
BuildKit state, while broad prune commands cannot be safely scoped to this probe's
temporary data. Any future opt-in command must present Docker's preview/accounting,
require explicit confirmation, and never include volumes by default. Until a
targeted cache policy and live evidence exist, manual Docker commands remain the
supported path.

## Hotspot triage

- **Host memory:** M15 already measured natural host-visible reclamation and
  rejected automatic ballooning; no new policy is justified by the current data.
- **Port polling:** M14 reduced the fallback poll from 30 to 6 wakeups per minute;
  event-driven reconciliation remains the fast path.
- **Disk:** the M16 result measured sparse provisioning and Docker accounting but
  not guest deletion through discard. This is the Stage 8 first experiment.
- **Filesystem and build performance:** existing M13/M14/M16 methodology gives
  the regression seams; no hotspot should be changed until a fresh runner result
  identifies a regression or material cost.
- **Virtualization startup:** the known `VZErrorDomain 1` transient is tracked as
  host state by R1, not treated as a performance optimization target.

This keeps the first Stage 8 diff to an evidence harness and avoids changing
runtime behavior before a measured hotspot exists.

## Current evidence

- Local execution host: not used for live validation; it does not establish
  macOS APFS or Virtualization.framework behavior.
- Existing M16 evidence proves sparse/APFS clone behavior and Docker accounting,
  but did not measure guest deletion through `fstrim` to host allocation.
- The self-hosted workflow is added; no runner result is claimed until its artifact
  is inspected.

## Next

Run the manually dispatched workflow, inspect `reclamation.csv` and `fstrim.log`,
then characterize image deletion and build-cache cleanup separately. Only after
that evidence should Stage 8 decide whether a narrow opt-in cleanup helper is
warranted.
