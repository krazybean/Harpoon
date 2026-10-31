# B8.3 controlled live failure and recovery — 2026-10-31

## Verdict

**CONDITIONAL PASS.** The user-reported L1–L4 live runs completed under the B8.3
controls against the real Harpoon Docker socket. The shipping deployment gate stayed
closed. No Harpoon VM lifecycle operation was performed for this work, and no live
container was started, stopped, or pruned by this repository change.

Evidence source: `/Users/juancastro/harpoon-b83-evidence/`, preserved outside the
repository. The results below record the supplied live evidence; this document was
not produced by rerunning those live commands.

## Scenarios

### L1 — failing build

- Deployment `dep_01M68Y6DSMH762Z2XTQ43B2GHG`, application
  `app_01M68Y6DSMH762Z2XTQ43B2GHF`.
- Buildx failed with exit 1 after the source step.
- Preliminary check, cutover, final check, and recovery were skipped.
- The retained image intent was reconciled as absent; no container or port lease
  remained.
- Before/after inventory comparison was empty.

### L2 — health check never passes

- Deployment `dep_01M68YT587CJFMNPXBJ4XX28CZ`, application
  `app_01M68YT587CJFMNPXBJ4XX28CY`.
- Build succeeded; preliminary health failed after the 60-second engine budget:
  117 attempts, 0/2 consecutive successes, HTTP 404 without the expected marker.
- The preliminary container and port `39000` were removed; the built image was
  retained, then `cleanup-image` removed it by its recorded id.
- Before and post-cleanup inventories matched exactly.

### L3 — interrupted deployment and recovery

- The interrupted run left deployment `dep_01M690AVB14N4XWFPTRW7RD5AQ` with its
  preliminary container and port `39000`; the container was running and labelled
  for Harpoon.
- `recover` marked the deployment interrupted without contacting Docker.
- `reconcile --apply` removed the recorded preliminary container and lease, then
  `cleanup-image` removed the retained image by recorded id.
- The final inventory matched the pre-run inventory exactly.

### L4 — image deletion conflict

- Deployment `dep_01M690ZPS7V6VCG6736WFXXW0Q`, application
  `app_01M690ZPS7V6VCG6736WFXXW0P`; image
  `sha256:6bf984c602660df1d82e8961252dc22c9e30091cfbfbcc7740e10b7e74272ce7`.
- A second tag was added to the same image. A direct live Docker API delete by id
  with `force=false&noprune=true` returned HTTP **409** and Docker's message said
  the image was referenced in multiple repositories.
- The extra tag was removed only after verifying the exact image id and tag count.
- `cleanup-image --apply` then removed the image by id; the final inventory matched
  the baseline exactly.
- **Gap:** the real Docker HTTP 409 was observed, but the Harpoon
  `DockerRefused` typed journal transition was not tested against the live Engine.
  The typed transition remains covered by simulated-Docker tests.

## Exact inventory restoration

For all four scenarios, the supplied inventory diffs were empty. The restored
baseline was:

- 9 pre-existing exited containers, all `harpoon=no`.
- 6 pre-existing images: `alpine:latest`, `alpine:3.22`, `postgres:16`,
  `hello-world:latest`, and two `stage8-reclamation` images.
- 1 pre-existing anonymous local volume.
- No published Docker port mappings and no test-port listener remaining.

The Harpoon process was not restarted. L4's final journal still records the failed
deployment and the image cleanup attempt/result; the live HTTP 409 itself was
captured separately and is not evidence of a typed `DockerRefused` journal record.

The shipping deploy gate remains closed.
