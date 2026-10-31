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
5. ~~Context size is measured before `.dockerignore`~~ — **changed in B4:** measured after the `.dockerignore` rules Harpoon can interpret with certainty; see "B4" below. It can still only over-count.
6. **No release replacement:** `stop_current` errors, `restore_previous` returns `NotAttempted`.
7. **Volumes are rejected, not managed.** An image whose `Config.Volumes` is non-empty (inherited declarations included) is refused before any container intent or create, because Docker would create an anonymous volume the ledger does not track. Containers are still removed with `v=false`; a later milestone must add volume ownership before this restriction can be lifted.
7a. **Image tags are never taken over.** `build()` inspects the intended tag first and fails closed if anything holds it (foreign, another deployment's, or this deployment's own); no intent is recorded and Buildx does not run. A race between that check and the build (something else creating the tag in the interval) is not prevented — Buildx would move the tag — and is only detected afterwards by the label check; closing it needs a build into a unique temporary tag plus an atomic retag, which is not part of this slice.
8. **Health probes run on the host** against the published port; whether that path matches what a user's browser
   reaches through Harpoon's forwarder is unverified. B4 adds an optional response marker (below), but the deployed
   application must emit it and the forwarder path itself is still B7.
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

## B4 — source and network boundary hardening
Branch `feature/deploy-b4-boundaries`. Same constraints as B3: no live socket, no real build, container or deployment.
`deploy` is untouched (124 tests); `deploy-docker` goes from 68 to 113 tests (45 new in `tests/b4_boundaries.rs`).

### Folder confinement (`src/confine.rs`)
- The selected project root, the build context and the Dockerfile are resolved with `canonicalize`. The context must lie inside the root
  (`DockerConfig::project_root`, default: the context itself); the Dockerfile must lie inside the context. The check is component-wise, so
  `/proj-evil` is not inside `/proj`.
- A Dockerfile name must be a plain relative path: absolute paths, `.`/`..` parts, empty names and NULs are refused before touching the disk.
  Symlinks are judged by where they lead (file links, directory links, chains); a link that stays inside is fine and Buildx is given the
  **resolved** path. A dangling link is "missing", not "escaped".
- Missing root/context/Dockerfile, wrong kind (file where a folder is expected and vice versa), permission denied (file, unsearchable folder,
  unreadable ignore file) and over-size text files (>1 MiB) are separate errors; none is read as "no ignore file".
- The check runs in `prepare_source` and **again immediately before the build**; a Dockerfile swapped for an escaping link in between is caught
  (test). Residual: a swap after that second check and before Buildx opens the file cannot be excluded from user space. B7 option: build from
  a private, `O_NOFOLLOW` copy of the confined context in the scratch directory.
- Not changed: the size walk never follows symlinks (tested, including a link loop and a link to a large outside folder). Buildx/BuildKit does not
  follow links out of the context either; Harpoon does not rely on that, it relies on the checks above.

### `.dockerignore` and the 256 MB limit (`src/ignore.rs`)
- Which file: `<Dockerfile>.dockerignore` if present, else `.dockerignore` in the context (BuildKit's precedence).
- Rule: **never under-count.** The limit is applied to the tree after the exclusions Harpoon understands with certainty: literal segments,
  `*` inside a segment, whole-segment `**`, anchored at the context root as Docker's are (`node_modules` does not match `sub/node_modules`;
  `*.log` does not cross folders). A pattern with `?`, `[..]`, `\`, `{`, `.`/`..` segments or `**` inside a segment excludes nothing for sizing.
  Any `!` exception line turns filtering off entirely (it could re-include an excluded path). Notes are written to the deployment log.
- The Dockerfile and the ignore files are counted whatever the rules say, as Docker always sends them; an ignored folder that contains one is
  still walked.
- The limit is still enforced by Harpoon's own walk, not by Buildx; the real transfer size after Buildx applies its own matcher is not observed.

### Port allocation
- **Finding and fix:** `GET /containers/json` reports a published port only while the container is running, so a *stopped* container that is
  configured to publish 38000 was invisible to allocation despite the doc comment in `ports.rs` saying "running or stopped". The backend now
  inspects every non-running container and reads `HostConfig.PortBindings`; an unreadable or malformed field fails allocation rather than being
  treated as "no ports" (tests; the fake mirrors the real shape).
- Tested against real loopback listeners (`BindProbe`, loopback-only and wildcard), running and stopped Docker mappings, two successive
  independent deployments (distinct application ports; the test port is reused because it is released before cutover), a failed deployment whose
  port is released and handed out again, a port taken between the check and the start (the contested port is never probed, one replacement,
  a second collision fails and cleans up) and the ledger's own leases (pure tests in `deploy`).
- Journaled leases from *other* deployments are not visible while the journal is per-run memory; cross-process lease visibility is B5.

### Health probing
- `TcpHttp` now has a single deadline for the whole exchange (a server that drips one byte at a time can no longer outlast the attempt timeout),
  refuses oversized or unterminated header blocks, never follows redirects.
- **Response marker** (`ResponseMarker`, `DockerConfig::health_marker`): an exact header value or a body substring. A response without it is a
  *failed attempt* whatever its status; a transport that cannot check a marker (the default for test doubles) fails closed. Without a marker,
  an unrelated listener answering 200 on the leased port is indistinguishable from the application — a test records that on purpose.
- Tested with disposable in-process servers: wrong/old marker, missing marker, prefix of a marker, body vs header marker, redirect (not healthy,
  target never contacted), accept-then-hang-up, silent server, drip server, oversized and truncated headers, container stopped mid-check while the
  port still answers, and a deployment-level run where an unrelated process holds the first test port.
- **What this does not show.** These servers listen on loopback in the test process. Docker in the fake never binds anything. Nothing here
  exercises Harpoon's real port forwarder, Docker's userland proxy, or the macOS-to-VM path. The marker is evidence about *who answered the probe*,
  and only if the application emits it.
- **Real-runtime evidence required (B7), unchanged in kind and now more precise:** (1) the container's inspected `NetworkSettings.Ports` /
  `HostConfig.PortBindings` equal the lease; (2) the host-side listener on that port belongs to the forwarder for this VM (process owner, loopback
  only); (3) a per-deployment marker, generated by Harpoon, handed to the disposable app (e.g. through its environment) and returned in a response
  header — mismatch fails; (4) negative controls: an unrelated listener on a neighbouring port, and a stale listener holding the leased port;
  (5) the same check repeated after cutover through the path a user's browser takes.

### Isolation review
- **Audit (tests):** beside running, stopped and other-deployment-labelled containers and foreign images, a successful deployment and four failing
  ones (final check, preliminary check, two port collisions, a foreign container squatting on our name) issue no mutating request that mentions any
  foreign id or name; every foreign object is still present in its original state; every mutation is one of create / start / stop / remove container
  or remove image; container removal is by full id with `force=false&v=false`; image removal is `force=false&noprune=true`. A foreign image on our tag
  is neither overwritten nor removed. Created containers carry no `Binds`, `Mounts`, `Volumes`, `VolumesFrom`, host network, privileged flag, extra PID
  namespace or devices. The anonymous-volume rejection is unchanged and its tests are untouched.
- **Image-tag check/build race — safe approach for B7.** Today: inspect tag → intent → Buildx; if something creates the tag in between, Buildx
  moves it and only the post-build label check notices. Concrete fix, no retag:
  1. Generate the tag from 128 bits of fresh randomness per attempt (`harpoon/<app>:<deployment>-<32 hex>`), record it in the Image intent before
     the build. Nothing can pre-create or collide with a name that did not exist when the intent was written; the existing "tag must be absent"
     check stays as a second line.
  2. Pass `--iidfile` (in Harpoon's scratch directory) and treat the **image ID** as identity from then on: after the build require
     `inspect(tag).Id` to equal the ID Buildx reported, then check labels and volumes, and remove **by ID only**, never by tag.
  3. Do not retag to a friendly name: the tag API moves an existing tag silently, which would reintroduce the race. A friendly name belongs
     in the ledger/UI, not in Docker.
  4. Must be verified on the real engine in B7: `--iidfile` semantics (image ID vs index digest) under the containerd image store versus the classic
     store, and that the ID printed is the one `--load` stored.
  Not implemented here because it changes the build contract and cannot be validated without a live engine.

### Evidence
| | |
|---|---|
| `deploy` | 124 tests, unchanged |
| `deploy-docker` | 113 tests (45 B4 + 46 backend + 15 wire/API + 7 units); fmt and clippy `-D warnings` clean; three repeated full runs without a failure |
| Mutations (B4) | 19 injected; 17 caught: Dockerfile-escape check removed, context-escape check removed, `..` allowed, `!` not disabling filtering, patterns matching at any depth, always-sent files not kept, pruning a folder that holds one, marker check always passing, per-read instead of overall deadline, stopped-container ports not read, re-confinement before the build removed, truncated headers accepted, default transport ignoring the marker, symlinks followed in the size walk, exception lines kept as patterns, sibling `Dockerfile.dockerignore` ignored, ignore-file escape allowed. Two did not change behaviour: "absolute path allowed" (the plain-relative check rejects the root component anyway, so the explicit test is redundant defence) and "`#` not treated as a comment" (the line becomes a literal pattern that matches nothing). One survivor became a test fix (the truncated-header case; a test edit had silently not applied). |

### Gates still open
- **B5:** durable journal (fsync, torn tail) and cross-process lease visibility; until then port leases of other deployments are only seen through Docker.
- **B7:** everything under "Real-runtime evidence required", the tag-race change above, LAN-isolation of the forwarder, a real-engine capture to replace
  the documentation-derived fake, the private-copy build option for the source swap window, guest-side disk space.

## B7 phase 1 — readiness (offline)
No live Docker, VM or real build. Tests: `deploy-docker` 154 (116 preserved + 32 in `b7_readiness.rs` + 6 in `b7_fixture.rs`); 20 mutations of the new gates, 20 caught.

- **Image identity.** Each build attempt draws 128 bits (`random_hex`, OS entropy; injectable) and tags `harpoon/<app>:<dep>-<32 hex>`. The Image intent is journaled before the
  tag is checked absent and before Buildx runs. Buildx gets `--iidfile <scratch>/iid-<nonce>`; the reported ID must be `sha256:` + 64 lower-case hex. The backend inspects the image **by that
  ID**, requires the answer to carry the same ID and all three ownership labels, records the receipt (= that ID) at once, and only then checks that the tag still names the same image
  (a moved or vanished tag fails the build, nothing runs, our image stays recorded and retained, the foreign image is untouched). Containers and the volume check use the **ID**, never
  the tag; image removal was already by verified ID (new test: the tag now names a foreign image — only ours is removed, by ID). Collisions and a retag race are tested; with the nonce the
  residual check/build window of B3/B4 is closed unless the randomness source is broken (an all-zero source is exactly what the mutation test checks for).
- **Health attribution.** `DockerConfig::attribution = PerContainer`: each container gets its own random value in `HARPOON_DEPLOYMENT_MARKER`; a response counts only with the same value in
  `X-Harpoon-Deployment-Marker`. Never journaled and never given to Buildx. Missing, wrong, unrelated-listener and stale (previous container's) markers fail; a transport that cannot check
  markers fails closed. Before probing, `NetworkSettings.Ports` must show exactly the container port published on exactly the leased host port (no extra port, no specific non-wildcard
  address); after a healthy verdict it is re-read, and the host-listener check (below) must pass, or the healthy response is reported as untrustworthy.
- **Forwarding.** See `DEPLOY_B7_FORWARDING_AUDIT.md`. `forwarder.rs`: strict `lsof -F` parser, `verify_forwarder` (exactly one listener, `127.0.0.1`, expected pid and command), `LsofCheck`.
- **Source safety.** The confined context is copied once into `<scratch>/ctx-<nonce>` (0700) and Buildx builds from the copy. Per file: `lstat`, open, `fstat` the open handle, device and inode must
  match; links are recreated, never followed; sockets/FIFOs/devices are skipped and reported; the copy is capped at the 256 MB limit and honours the `.dockerignore` rules Harpoon understands. The
  copied Dockerfile must equal the one confined at build start and name the same base images as the preflight, or the build stops. The snapshot and the ID file are removed afterwards.
  This closes the swap window between the last check and Buildx; the copy itself is guarded per file as described.
- **Guest free space.** `guest.rs` fixes the rule (fail closed: error, zero, short or no probe all refuse; required = 2 GiB reserve + context size). **No measuring probe exists**: the Docker API reports usage, not
  free space, and Harpoon has no guest disk report. The CLI therefore refuses every live deployment at its first step until Phase 2 provides one (specified in `DEPLOY_B7_LIVE_ACCEPTANCE.md`).
- **Live gates.** `DockerConfig::live_gates` (set by the CLI for every non-simulated deployment) makes the backend refuse unless attribution is on, a host-listener check and a guest probe are configured.
- **Fixture.** `deploy-docker/fixtures/http-marker` (`alpine:3.22`, busybox `nc -e`, `/health` returns the marker; 404 elsewhere; 503 without a valid marker). Tested offline under `/bin/sh` and with Harpoon's own preflight and HTTP client; **never built or run in Docker**.
- **Behavioural changes to existing contracts:** `image_tag(app, dep, nonce)`; `BuildRequest.iidfile`; builds run from a snapshot (cwd is `<scratch>/ctx-…`); Buildx sees the container-ID flow above. Existing tests were updated for these, none weakened.

Not done / not claimed: LAN isolation; behaviour on a real host, real `lsof`, real Docker/Buildx (`--iidfile` semantics under the containerd image store); teardown of a successful deployment; the guest measurement.
