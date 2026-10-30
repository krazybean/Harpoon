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
| `deploy-docker` | 68 tests (46 backend, 15 wire/API, 7 units), fmt + clippy clean, repeated full runs without a failure |
| Mutation checks | 22 deliberate defects injected, 22 caught — 18 in the first review plus the four isolation guards added after it: tag-in-use check removed, Harpoon-labelled tag allowed to be overwritten, volume check removed, volumes ignored by the parser. First review's 18: (removal without label/name/id check, forced delete, backend accepting simulated evidence, evidence not marked simulated, unlabelled image accepted, lease not released after collision, Buildx inheriting the environment, base-image check skipped, receipt not carrying Docker's id, `HostIp` added, swap allowed, preliminary cleanup disabled, unlistable runtime treated as absent, timeout not killing the process group, `# syntax=` allowed, name conflict ignored). Four survived or didn't compile on the first pass and led to test fixes (fake engine now resolves unique id prefixes like Docker; the hung-build test now checks the grandchild process). |

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
7. **Volumes are rejected, not managed.** An image whose `Config.Volumes` is non-empty (inherited declarations included) is refused before any container intent or create, because Docker would create an anonymous volume the ledger does not track. Containers are still removed with `v=false`; a later milestone must add volume ownership before this restriction can be lifted.
7a. **Image tags are never taken over.** `build()` inspects the intended tag first and fails closed if anything holds it (foreign, another deployment's, or this deployment's own); no intent is recorded and Buildx does not run. A race between that check and the build (something else creating the tag in the interval) is not prevented — Buildx would move the tag — and is only detected afterwards by the label check; closing it needs a build into a unique temporary tag plus an atomic retag, which is not part of this slice.
8. **Health probes run on the host** against the published port; whether that path matches what a user's browser
   reaches through Harpoon's forwarder is unverified.
9. **Container logs are fetched only on failure**; a successful deployment keeps none beyond the build output.
10. **Timeouts use the real clock** inside the backend (step budgets from `StepIo`), while the engine's own clock is
    injectable; a fake engine clock does not advance backend deadlines.
11. **No secrets, networks or user env injection** beyond `DockerConfig::env`; Buildx secrets remain Milestone C.
12. Guest registry DNS (G1), Intel guest (G3), clean-Mac gate (G4) from `DEPLOY_BUILDX_VALIDATION.md` are unchanged.

## Added by the isolation review (this PR)
- **Tag collisions:** foreign image, another deployment's image and this deployment's own image holding the tag each stop the
  build before Buildx runs, leave the existing image and tag untouched, remove nothing and record no image intent. The tag is
  checked before the intent, and the intent is still durable before the build's result is read.
- **Volumes:** declared volumes (e.g. `/data`, inherited ones included) fail the preliminary check with no container intent,
  no create request, no volume request, and an unrelated container untouched. A malformed `Config.Volumes` is an error, not a pass.

## B7 acceptance gate — the health response must come from *this* deployment
Not verified. Nothing in B3 shows that a successful HTTP response came from the intended container rather than an unrelated
listener on the same host port (a host process, another forwarder, a stale Docker proxy). Before B7 can call a deployment healthy:
1. **Mapping check.** Read the container back (`inspect`) and require `NetworkSettings.Ports["<cport>/tcp"]` to contain exactly the leased
   host port, and `State.Running`, immediately before and after the probe; require Docker's own published-port listing to agree.
2. **Forwarder check.** Verify how the port reaches the host: which process owns the host-side listener (e.g. `lsof -nP -iTCP:<port> -sTCP:LISTEN`
   must name Harpoon's forwarder, not another process), that it is bound to loopback only, and that it forwards to this VM's published port. Confirm
   the host's allocation check and the forwarder agree on ownership.
3. **Unique marker (end-to-end, disposable app).** The test application returns a per-deployment random marker (e.g. an `X-Harpoon-Probe` header
   or body string passed in as an environment variable and also recorded in the ledger journal). The probe must see exactly that marker; a
   200 without it is a failure. Negative control: start an unrelated listener on a *different* test port that answers 200 with another marker
   and prove the probe on the leased port does not accept it; and prove a stale/foreign listener on the leased port is rejected (marker mismatch).
4. **Requires** a disposable application image, the live runtime, and written authorization for it (B7). **This has not been run against the live
   runtime and nothing here claims it has;** B3's fake cannot show it, because the fake has no network path.
