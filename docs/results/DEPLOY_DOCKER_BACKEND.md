# Milestone B3 — Docker backend against a recorded-response fake

Status: implemented on `feature/deploy-docker-backend`; **not merged**. No real VM, Docker build,
container, or workload was touched; no test contacts a live Docker endpoint.

## What exists
- `deploy-docker/` (new crate, path-depends on `deploy/`): JSON reader, HTTP/1.1-over-Unix-socket client, Docker API
  calls, `DockerBackend`, bundled-Buildx runner, Dockerfile/context/disk preflight, real HTTP health probe.
- Engine changes in `deploy/` (closing B1 gaps): the preliminary check's container and test-port lease are removed
  **before cutover** (a removal that fails stops the deployment with the old release untouched); `StepIo` gained
  `cleanup_attempt` / `cleanup_result` / `release_lease`; `Backend::accepts_simulated_evidence()` (default `false`,
  only fakes say `true`) makes the engine refuse simulated evidence from a real backend; an unlistable runtime during
  the pre-cutover cleanup is recorded as a **failed** cleanup, never "already absent".
- One existing B1 test encoded the old contract (preliminary container removed only after the deployment ended) and was
  rewritten to the new one: `a_failed_cleanup_after_a_successful_deployment_is_still_reported` →
  `a_failed_removal_of_the_preliminary_container_stops_the_deployment_before_cutover`. All other tests are unchanged.

## Evidence
| | |
|---|---|
| `deploy` | 124 tests (118 + 6 new), fmt + clippy `-D warnings` clean, still no `std::net`/`std::process`/`UnixStream` in `src` |
| `deploy-docker` | 62 tests (40 backend, 15 wire/API, 7 units), fmt + clippy clean, 3 consecutive full runs without a failure |
| Mutation checks | 18 deliberate defects injected, 18 caught (removal without label/name/id check, forced delete, backend accepting simulated evidence, evidence not marked simulated, unlabelled image accepted, lease not released after collision, Buildx inheriting the environment, base-image check skipped, receipt not carrying Docker's id, `HostIp` added, swap allowed, preliminary cleanup disabled, unlistable runtime treated as absent, timeout not killing the process group, `# syntax=` allowed, name conflict ignored). Four survived or didn't compile on the first pass and led to test fixes (fake engine now resolves unique id prefixes like Docker; the hung-build test now checks the grandchild process). |

Covered by tests: success path with intent→create→receipt ordering stamped against the fake engine's requests; every
DELETE preceded by an inspect of the same id and a stop of a running container; name collisions (foreign container
untouched and reported as refused; leftover container labelled for this deployment adopted and removed); port steering
by host listeners and Docker mappings; one replacement after a port collision, a second collision fails and cleans up;
removal refused on missing/foreign labels, renamed container, id-prefix mismatch; removal racing a vanishing container;
failed stop / 409 on delete / timeout; a create whose reply is lost (adopted by label); Docker closing the connection
mid-operation; malformed JSON and missing fields; truncated and chunked bodies; garbage that is not HTTP; slow engine
timeouts; build failure, hung build killed at the deadline (process group), redaction and bounded build logs; missing /
non-executable / symlinked Buildx; non-local base image, `# syntax=`, variable base images, oversized context, low disk;
a crash at **every** journal write of a full deployment followed by label-based recovery with a new backend instance,
with an unrelated container and the base image untouched.

## Remaining integration gaps (for B4–B7)
1. **Fake ≠ real.** Response shapes, error texts (`port is already allocated`, 409 messages), log framing and prefix
   resolution come from the Docker Engine API docs, not from a captured live exchange. Re-verify against the real
   engine in B7 (with authorization).
2. **LAN exposure not verified.** Publishing without `HostIp` makes Docker bind the wildcard address on the host side of
   whatever forwards to the VM. The approved requirement (host listener stays loopback-only, LAN isolation verified) is
   untested here; it needs the real Harpoon port forwarder.
3. **Journal is memory-only** (`MemoryJournal`); the durable file journal (fsync, torn-write truncation) is B5. Crash
   tests prove the *protocol*, not disk durability.
4. **Disk preflight is host-side** (`df` on the scratch directory). Free space inside the guest is not checked.
5. **Context size is measured before `.dockerignore`**, so it can only over-refuse.
6. **No release replacement:** `stop_current` errors, `restore_previous` returns `NotAttempted`.
7. **Anonymous volumes** are never removed (`v=false`); a base image that declares `VOLUME` can leave one behind.
8. **Health probes run on the host** against the published port; whether that path matches what a user's browser
   reaches through Harpoon's forwarder is unverified.
9. **Container logs are fetched only on failure**; a successful deployment keeps none beyond the build output.
10. **Timeouts use the real clock** inside the backend (step budgets from `StepIo`), while the engine's own clock is
    injectable; a fake engine clock does not advance backend deadlines.
11. **No secrets, networks or user env injection** beyond `DockerConfig::env`; Buildx secrets remain Milestone C.
12. Guest registry DNS (G1), Intel guest (G3), clean-Mac gate (G4) from `DEPLOY_BUILDX_VALIDATION.md` are unchanged.
