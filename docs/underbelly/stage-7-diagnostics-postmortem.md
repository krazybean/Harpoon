# Stage 7 Diagnostics Gate Postmortem

Status: **CLOSED**

Freeze tag: `stages-0-7-closed`

Validated runtime commit: `8cfbaec49f1ae6acfe757f9225de5b908266364b`

## Summary

Stage 7 initially reported an environment block while leaving Stage 6 validation containers behind. The first cleanup patch fixed the shell-level ownership and exit-path problems, but live testing exposed a second defect below the gate: Harpoon stopped the VM without flushing the guest filesystem. Docker deletions succeeded in the running guest, but the deleted state was lost when the disk was detached and the containers returned after restart.

The final fix moved the guest `sync` request to the host-side `harpoon stop` command, before SIGTERM and VM shutdown. A rebuilt runtime then passed Stage 7 live, and a subsequent Harpoon restart showed zero containers.

## Failure chain

1. Stage 6 created a correctly labeled probe: `validation=stage6`, `stage=6`.
2. Stage 2 correctly treated that pre-existing container as foreign and blocked; its strict zero-container State A was preserved.
3. The original cleanup regression skipped prior gates, so it never covered “prior gate blocks, live Stage 6 probe still runs.”
4. The shell cleanup was hardened to use a single exit finalizer, the returned container ID, explicit absence verification, and preserved exit status.
5. Live testing then showed that both Stage 6 and Stage 2 deletions were lost across Harpoon stop/start.
6. The first runtime sync attempt ran after the VM had entered shutdown and could not reach guest management reliably.
7. The final runtime fix syncs the guest before sending SIGTERM.

## Root causes

### Gate lifecycle

Cleanup was not originally modeled as part of the gate’s final outcome. The regression suite did not compose prior-gate blocking with the live probe, and cleanup failures could be obscured by shell exit/trap behavior.

### Runtime persistence

Harpoon detached the writable guest disk without a guaranteed guest filesystem flush. Docker’s API response was not sufficient evidence that the deletion would survive VM shutdown.

## Fixes

- Stage 6 cleanup owns only its labeled probe and runs from one `EXIT` finalizer.
- Cleanup uses the Docker create result ID rather than relying only on the generated name.
- Cleanup failure or Harpoon-stop failure changes the gate result to failure.
- Stage 6 regression covers normal pass, cleanup failure, already-running runtime preservation, stop ordering, and blocked prior-gate composition.
- Host-side `harpoon stop` requests guest `sync` before SIGTERM.
- Stage 2 ownership and strict State A behavior remain unchanged.

## Evidence

- Stage 6 cleanup regression: passed.
- Stage 2 ownership regression: passed.
- CLI parity regression: passed.
- macOS Swift runtime build: passed.
- Live Stage 7 gate: `STAGE_7_GATE_PASS`, exit `0`.
- Post-gate Harpoon restart: Docker container list empty.
- Working tree: clean.
- Freeze tag: `stages-0-7-closed` pushed to `origin`.

## Required acceptance for future gates

Each live gate that starts or stops Harpoon must verify state after a complete stop/start cycle. A cleanup message from the running Docker daemon is not sufficient evidence of durable cleanup. The final assertion must be made after the runtime is available again, with the exact ownership boundary preserved.

Stage 8 has not started.
