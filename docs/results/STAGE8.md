# Stage 8 — Performance Investigation

Status: **8B/8D VALIDATED — guest discard reaches APFS backing allocation; auto-prune remains deferred**

## Scope

Stage 8 starts with the unresolved M16 question: after Docker deletes data from
guest ext4, does host-visible allocation of Harpoon's sparse backing file decrease?
Guest `df` alone is not sufficient evidence.

The probe is `tools/stage-8-disk-reclamation-check.sh` and is dispatched by
`.github/workflows/stage-8-disk-reclamation.yml` on the existing
`self-hosted, macOS, harpoon` runner.

`tools/regression-stage8-disk-reclamation.sh` is the focused 8D check for probe
syntax, explicit root trim, unsupported-trim classification, test-image-only
cleanup, and failure-safe artifact upload.

## Safety contract

- Creates one uniquely named temporary image from a generated tar archive.
- Removes only that temporary image.
- Does not remove containers, volumes, user images, or user build cache.
- Records guest `fstrim` and virtio-blk discard-queue capability after deletion.
- Attempts explicit root-filesystem `fstrim /`; missing or rejected trim is
  `unsupported-or-rejected`, never a pass.
- Does not run `docker system prune` or broad `docker builder prune`.

## Measurement

`reclamation.csv` records, for baseline, image-created, image-removed, and
after-fstrim: logical backing-file bytes, allocated host blocks and block size,
and guest root used/free bytes. `discard-capability.txt` records the guest
`fstrim` path, root filesystem, and virtio-blk discard queue values. A drop in
guest free space without a corresponding drop in host allocated blocks is not
physical APFS reclamation.

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
- Run [38631020307](https://github.com/krazybean/Harpoon/actions/runs/38631020307)
  established that deletion restored guest free space while host allocation
  stayed unchanged; its `fstrim -av` attempt was rejected by BusyBox.
- The current probe uses explicit `fstrim /` and records discard capability; no
  public Virtualization.framework disk-image API in the current attachment path
  exposes a discard toggle.
- Run [38632009299](https://github.com/krazybean/Harpoon/actions/runs/38632009299)
  on commit `ad9626cb632eb6d667045182cf05eace490fdc1b` completed successfully.
  Artifact `stage-8-disk-reclamation` recorded:
  - `image-removed`: 11,054,560 allocated blocks × 4096 bytes; guest free
    space returned to the 843,849,728-byte baseline.
  - `after-fstrim`: 1,915,784 allocated blocks × 4096 bytes; guest
    `fstrim /` completed.
  - `discard-capability.txt`: ext4 root, `/sbin/fstrim`, 512-byte discard
    granularity, 34,359,738,368-byte maximum discard, zeroes-data false.
  This validates the guest-to-APFS discard path on the self-hosted runner.
- Stage 8D focused regression: `PASS Stage 8 probe safety and trim
  classification` via `bash tools/regression-stage8-disk-reclamation.sh`.

## Next

Keep `fstrim /` as a diagnostic experiment only; it does not justify automatic
pruning. Stage 8C remains deferred because Docker image and BuildKit prune
commands are broader than this test-owned image. Stage 8 is complete for the
8B investigation and 8D regression scope; no Stage 9 or UI work follows from
this result.
