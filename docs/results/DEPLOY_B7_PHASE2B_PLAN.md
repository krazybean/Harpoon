# B7 Phase 2B — preparation and the proposed (NOT executed) invocation

Status: **prepared, not executed.** Phase 2B (the first real build and deployment of the disposable fixture) needs the user's separate, explicit authorization. This document records the two local prerequisites,
the exact proposed command, the procedure around it, and what is still unknown. Nothing here touched the live runtime beyond the read-only commands listed under "Evidence".

## 1. Private Buildx resources — done
| | |
|---|---|
| Source | the B2 development artifact `…/Harpoon-deploy-b2/harpoon/build/docker-buildx` (produced by `harpoon/fetch-buildx.sh`; nothing downloaded or substituted) |
| Version | `github.com/docker/buildx v0.38.1` |
| Pin | arm64 slice sha256 **`6f20ba2437865311ef99015bf5d1039c3c8e4d2177c1c64aa41b134b5ecd3012`** — equal to the pin in `fetch-buildx.sh`, to the line for `buildx-v0.38.1.darwin-arm64` in the cached `checksums-signed-0.38.1.txt`, and to the cached download slice; checked with `lipo -thin arm64` |
| Architecture | `arm64` only (`lipo -archs`) |
| Signature | `codesign --verify --strict`: valid; Authority Developer ID Application: Docker Inc (9BNSXJN65R), hardened runtime (`flags=0x10000`), secure timestamp 2026-10-20 |
| Whole-file sha256 | `70dbccab5e0d261353a802315651f6827cee90bb22f5f3eeb39ffc7bbe55e075` (differs from the pin only because the fetch script `lipo`-wraps the verified slice) |
| Private copy | `/Users/juancastro/.harpoon-b7-prep/Resources/harpoon/bin/docker-buildx` — directories `0700`, a regular file (not a symlink), `0755`, **byte-identical** to the source (`cmp`, same sha256), signature still valid |
| Resolved by the CLI | `doctor --resources-dir /Users/juancastro/.harpoon-b7-prep/Resources` → `[ok] bundled Buildx: …/harpoon/bin/docker-buildx (the only Buildx Harpoon will run)`; the whole `doctor` is 6 ok / 0 warnings / 0 failures, exit 0 |
| Not touched | `/Applications/Harpoon.app` (its `harpoon/bin` still holds only `harpoon`); the live runtime; the proposed B7 runtime directory `~/.harpoon-b7-runtime` (still absent) |

## 2. The experimental CLI — built, not run for deployment
- Name: `harpoon-deploy-b7-experiment`. Path: `/Users/juancastro/Documents/Github/Harpoon-b7-experiment/deploy-cli/target/release/harpoon-deploy-b7-experiment`, sha256 `23905f40404abafd9b77e60e8330321710f4ae20746eccba11d0a08aeeaed702`.
- Build revision: **`236ccece6f0e417039c74211d2f2d9886f8af921`** on the **local-only** branch `experiment/b7-live-gate` (one commit on top of `main` `89b609c`; never pushed — `ls-remote` shows nothing — and its push remote is set to an invalid value). It prints its revision with `--b7-experiment-info` and a banner on every run.
- What it changes: one new file (`deploy-cli/src/experiment_main.rs`) and one `[[bin]]` entry. The file runs the same library with `Gate { live_deploy_enabled: true }`. Everything else — the exact authorization phrase, explicit sockets, ownership checks, journal, `--forwarder-pid`, `--mgmt-socket` — is the library's, unchanged.
- **Not in `main`:** `git grep "live_deploy_enabled: true"` finds the gate-opening code only in that one file on the experiment branch; `origin/main` has zero occurrences in `deploy-cli/src`; the normal binary built in the same tree contains no "EXPERIMENTAL" banner string.
- **Shipping gate:** the ordinary binary `deploy` (real sockets, no authorization phrase, a throwaway runtime path) → "live deployment is disabled in this build (Milestone B7 gate). Nothing was read, written or contacted.", exit 6, nothing created. The real-binary test `a_real_binary_invocation_cannot_deploy_live` passes.
- **Phrase requirement with the gate open:** shown by the library tests built with `Gate { live_deploy_enabled: true }` (`live_deploy_is_refused_before_anything_is_read_written_or_contacted`: no phrase → exit 6, a wrong phrase → exit 6, nothing created, no Docker request). The experimental binary's own `deploy` was deliberately **not** executed.

## 3. The proposed Phase 2B invocation (do not run without authorization)
Variables (all absolute): `PREP=/Users/juancastro/.harpoon-b7-prep`, `RT=/Users/juancastro/.harpoon-b7-runtime` (must not exist yet), `FIX=<repo>/deploy-docker/fixtures/http-marker` (Dockerfile sha256 `c16347a2…565e`, `srv.sh` `fd798e0c…c03d`),
`EVID=<a new private directory outside Harpoon's runtime and journal>`, `X=…/target/release` of the experimental build, `S=/tmp/harpoon-docker.sock`, `M=/tmp/harpoon-mgmt.sock`.

```sh
# 0. the binaries are the ones that were prepared
shasum -a 256 "$X/harpoon-deploy-b7-experiment"   # must equal 23905f40…d702
"$X/harpoon-deploy-b7-experiment" --b7-experiment-info   # revision 236ccece6f0e417039c74211d2f2d9886f8af921
test ! -e "$RT"                                           # the journal directory does not exist yet

# 1. DISCOVER the forwarder pid now (never reuse an old value); all read-only
PID=$(cat "$HOME/Library/Application Support/Harpoon/runtime.pid")
/usr/sbin/lsof -nP -p "$PID" -a -U -Fn | grep -x 'n/tmp/harpoon-docker.sock'   # this pid holds the Docker socket
/usr/sbin/lsof -nP -p "$PID" -Fc        | grep -x 'charpoon'                    # and its command name is exactly "harpoon"
# also: the log has no HOST_FORWARD_TRY_FALLBACK / DISCOVERY_FAILED / FORWARD_COLLISION since the last start

# 2. BEFORE-inventory and read-only gates (both must succeed)
"$X/harpoon-deploy" inventory --docker-socket "$S" --lsof-path /usr/sbin/lsof --to "$EVID/before.txt"
"$X/harpoon-deploy" doctor --runtime-dir "$RT" --project-dir "$FIX" --container-port 8080 --health-path /health \
    --resources-dir "$PREP/Resources" --docker-socket "$S" --mgmt-socket "$M"                     # exit 0
"$X/harpoon-deploy" plan   --runtime-dir "$RT" --project-dir "$FIX" --container-port 8080 --health-path /health --docker-socket "$S"   # exit 0, "blockers: none"

# 3. THE DEPLOYMENT (the only mutating command) — only after the user's explicit authorization
"$X/harpoon-deploy-b7-experiment" deploy \
    --runtime-dir "$RT" \
    --docker-socket "$S" --mgmt-socket "$M" \
    --resources-dir "$PREP/Resources" \
    --project-dir "$FIX" --container-port 8080 --health-path /health \
    --forwarder-pid "$PID" --forwarder-command harpoon --lsof-path /usr/sbin/lsof \
    --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE

# 4. AFTER (read-only), compared with the before-inventory
"$X/harpoon-deploy" inventory --docker-socket "$S" --lsof-path /usr/sbin/lsof --to "$EVID/after.txt"
diff "$EVID/before.txt" "$EVID/after.txt"
"$X/harpoon-deploy" status --runtime-dir "$RT"
"$X/harpoon-deploy" export-journal --runtime-dir "$RT" --to "$EVID/journal-after.wal"
```
Expected, exhaustively, in the diff after a **successful** run: one new image (`harpoon=yes`, tag `harpoon/<app>:<dep>-<128 random bits>`), one new running container `hp-<dep>` publishing 38000, one `docker-port-mapping 38000`, one `host-listener 127.0.0.1:38000 pid=<PID> command=harpoon`; the nine containers, six images and one volume unchanged. After a **failed** run: at most one new image. The deployment leaves the application running; removal is `teardown` (separate authorization).

Flag choices that come from Phase 2A: `--forwarder-command harpoon` (the real lsof name; the default `Harpoon` would reject a correct listener); `--health-path /health` (the fixture answers nothing else); the pid from step 1, not from the earlier report; `--mgmt-socket` and the private `Resources` tree.

## 4. Before-inventory procedure and protection of existing workloads
1. `inventory` (step 2) is the contract: nine exited containers (`harpoon-ui-stack-test-20261015-*`, `harpoon-ui-preview-{alpha,beta}-1`), six images (`alpine:3.22`, `alpine:latest`, `postgres:16`, `hello-world:latest`, two `stage8-reclamation-*`), one anonymous volume, no mappings, no listeners in 38000–39099.
2. The deployment can only create what it journals and can only remove (at cleanup) objects whose ID, name and **all three** labels it recorded; none of the existing objects carries Harpoon's labels. The container create has no binds, mounts or volumes; the image volume check refuses images that declare volumes; the build uses `--pull=false`.
3. After the run, any line that was in the before-inventory and is missing or changed in the after-inventory is an immediate abort + report.

## 5. Abort conditions (stop, change nothing further, export the journal)
`DEPLOY_B7_LIVE_ACCEPTANCE.md` §3 applies unchanged. Specifically for this run: the experimental binary's revision or hash differs from the above · step-1 pid does not hold the Docker socket or is not named exactly `harpoon` · `RT` already exists · `doctor`/`plan` not exit 0 · the before-inventory differs from the nine/six/one baseline · the Harpoon log shows a fallback guest IP, a collision, or the pid changed · the build fails or exceeds its budget (a **failed build is an acceptable outcome**: it leaves at most an image intent and is cleaned by `reconcile`, never by hand) · the after-inventory differs in anything but the listed additions · a health response without this container's marker · any journal error · the user says stop.

## 6. Unknown until the first real build and deployment
- `--iidfile` content and `--load` tagging against the real engine (classic overlay2 store: the ID should equal inspect's `Id`; unverified).
- Whether BuildKit resolves `FROM alpine:3.22` from the local store with `--pull=false` **when the guest cannot reach a registry** (the earlier validation notes a guest-DNS gap). If it tries the network the build fails closed.
- Buildx's first-run behaviour with the empty scratch `HOME`/`DOCKER_CONFIG` and `DOCKER_HOST=unix:///tmp/harpoon-docker.sock` (builder creation, `docker` driver).
- `nc -lk -p 8080 -e /srv.sh` working in `alpine:3.22`; the container receiving `HARPOON_DEPLOYMENT_MARKER`; the `/health` header surviving the forwarder.
- The `NetworkSettings.Ports` of a running published container (`HostIp` values), the 409/500 error bodies, stop/remove timing.
- The real address `lsof` prints for the forwarder's listener (expected `127.0.0.1:38000`) and the ~10 s forwarder lag.
- `teardown` against the real engine; LAN isolation (a separate test, separately authorized).

## 7. Evidence for this preparation
Read-only only: file hashing/`codesign`/`lipo` on local files; `doctor` (exit 0, 6 ok), `plan` (exit 0, blockers none) against the live sockets through the ordinary and the experimental binaries (`doctor` only); `ps`/`lsof` for the pid check; the ordinary binary's gate refusal on a throwaway path.
No `deploy`, `recover`, `reconcile --apply` or `teardown --apply` was run; no image built; no container created or removed; Harpoon not restarted; the runtime directory not created; no LAN test.

## Recommendation
**Phase 2B: GO, conditional on the user's explicit authorization of the deployment itself.** Both blockers from Phase 2A are closed: a verified private Buildx tree that `doctor` accepts, and a separately named, unmerged, revision-stamped experimental binary while the shipping binary still refuses. Remaining risk is the unknowns in §6, which only the first real build can resolve, and each has a fail-closed outcome. Await the separate authorization.
