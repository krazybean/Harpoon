# Stages 0–1 Exit Gate

This is the barrier between the frozen Stage 0–1 baseline and any Stage 2
implementation. It is intentionally a single verdict: `STAGE_0_1_GATE_PASS`
is the only result that formally permits Stage 2 to begin.

Run from the repository root:

```sh
HARPOON_GATE_REPORT=/tmp/harpoon-stage-0-1-gate.log \
  bash tools/stage-0-1-gate.sh
```

The script does not alter `tools/validate.sh` or production runtime behavior.
It resolves an installed Node 20 binary explicitly, then runs the existing
repository validation gate unchanged. Node 22 is a failure, not a fallback.

## Required barriers

1. Git branch, `HEAD`, and working-tree state are recorded.
2. Existing repository validation passes under Node 20.
3. The live smoke is rerun on a permitting macOS host: VM start, Docker
   readiness, the actual `/tmp/harpoon-docker.sock`, the actual
   `/tmp/harpoon-mgmt.sock`, Docker API response, a basic Docker run, and
   management `exec` and shell smokes.
4. Runtime-anatomy observations are checked against the live socket and status
   facts; process-lifetime claims that cannot be observed remain inferred in
   `runtime-anatomy.md`.
5. Stale architecture, roadmap, and version metadata remains documentation
   debt or qualified historical evidence; it is not used as current truth.
6. Docker Engine remains authoritative for Docker/container semantics. Harpoon
   remains authoritative only at the macOS↔Linux boundary.

If the host cannot run live acceptance, the script records `SKIP` and fails the
single gate. That is a blocker to formal closure, not a pass.

## Ownership rule

This gate is a verification seam, not a container-runtime seam. It observes
Docker through Docker's API and observes Harpoon at the VM, socket, and
management boundary. It does not add container, containerd, shim, or `runc`
behavior.
