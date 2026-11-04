# B7 live acceptance checklist (Phase 2 — requires the user's separate authorization)

> **Final results:** the first live deployment, LAN-isolation test and teardown are recorded in [`DEPLOY_B7_LIVE_RESULTS.md`](DEPLOY_B7_LIVE_RESULTS.md) (**B7 first-deployment acceptance: PASS**; not general production readiness). The text below is the historical checklist and plan and is kept as written.

Nothing in this document has been run. Phase 1 (this PR) is offline only. **No step below may be performed without the user's explicit, separate authorization for that step.**

## 0. Preconditions — all must hold before step 1
- [ ] Written authorization from the user naming: the Harpoon runtime (Docker socket **and** management socket), the disposable application (`deploy-docker/fixtures/http-marker`), and each authorization point in section 5.
- [ ] A reviewed change opens `Gate::shipping()` for live deployment (a constant; nothing else opens it). **Not part of this PR or the one before it.**
- [x] Implemented and tested offline (this PR): guest free-space measurement through the existing management channel (`--mgmt-socket`); `teardown` for a succeeded deployment; the deployment-specific marker; image identity; the private source snapshot; the host-listener check.
- [ ] **Still open before a live run (cannot be closed offline):** captured real Docker API responses to replace the documentation-derived fake · the real `lsof` output format · the Harpoon app's process name and pid · what real busybox `df -B1`, `hostname` and `uname -r` print in the guest · `--iidfile` behaviour on the real engine · that `nc -lk -e` works in `alpine:3.22` · the LAN-isolation test.
- [ ] `cargo fmt/clippy/test` green for `deploy`, `deploy-docker`, `deploy-journal`, `deploy-cli` on the exact commit used.
- [ ] A fresh runtime directory (`0700`) used for nothing else; `runtime-kind` = docker.

## 0a. The invocation the fixture needs
The fixture answers `/health` only (404 elsewhere), and the CLI's default health path is `/`. A live deployment of the fixture therefore **must** pass `--health-path /health`, plus `--mgmt-socket M --forwarder-pid N` (and `--forwarder-command` if the app's process is not called `Harpoon`). (An offline test found this: deploying the fixture with the default path never becomes healthy.)

## 1. Inventories — before and after (read-only)
Capture each twice (before, after) into files that do not already exist:
| Inventory | How | Compare |
|---|---|---|
| Containers (id, name, state, Harpoon-owned?, published ports) | `harpoon-deploy inventory --docker-socket S --to before-docker.txt` | `diff before-docker.txt after-docker.txt` |
| Images (id, tag, Harpoon-owned?) | same file | same |
| Volumes (name, driver) | same file (`GET /volumes`, inventory only) | same — **must be identical** |
| Docker port mappings (running and configured) | same file | only the disposable deployment's ports may differ |
| Host listeners in 38000–39099 | `--lsof-path /usr/sbin/lsof` on the same command | only the leased ports, only `127.0.0.1`, only the Harpoon pid |
| Forwarding log | the Harpoon app log lines `HARPOON_GUEST_IP_*`, `HARPOON_PORT_FORWARD_*`, `HOST_FORWARD_*` for the run | no fallback-IP lines; `…LISTENING 127.0.0.1:<port>` for each leased port |
| Journal | `status`, then `export-journal --to <new file>` | final state matches the report |
Expected differences after a successful run, exhaustively: **one new image** (the built one, retained), **one new running container** (`hp-<dep>`), their published port; nothing else. After a failed run: one new image at most.

## 2. Every operation Phase 2 will perform
**Read-only:** `GET /_ping`; `GET /info` (guest identity and data root); `GET /containers/json?all=true`; `GET /containers/{id}/json`; `GET /images/json`; `GET /images/{ref}/json` (base image, tag, ID); `GET /volumes` (inventory only); `GET /containers/{id}/logs` (on failure); `lsof` (fixed arguments); **in the guest, through the management channel, exactly three commands: `hostname`, `uname -r`, `df -B1 <Docker data root>`** (literal argv, no shell, no Docker object created — nothing to journal).
**Changing Docker state (and nothing else):**
1. One build through the **bundled** Buildx (`--load`, `--pull=false`, three ownership labels, `--iidfile`, unique tag `harpoon/<app>:<dep>-<128 random bits>`), from a private snapshot of the project folder. Creates one image.
2. `POST /containers/create` ×2 — `hp-<dep>-pre` (test port 39xxx) and `hp-<dep>` (application port 38xxx): created **from the verified image ID**, labels ×3, `HARPOON_DEPLOYMENT_MARKER=<random per container>`, 256 MiB / 0.5 CPU / 64 PIDs, no swap, no restart policy, no `HostIp`, no binds, mounts, volumes, privileged, host network or devices.
3. `POST /containers/{id}/start` ×2.
4. `POST /containers/{id}/stop` then `DELETE /containers/{id}?force=false&v=false` for the preliminary container, after re-inspecting id, name and all three labels.
5. After observation, if the user authorizes it: `teardown --apply` (section 4): stop and remove the serving container by verified ID, remove the image by verified ID if nothing else uses it, release the lease.
No pull, prune, exec, network, volume-changing or image-removal call is made except by the teardown/reconcile procedures below, each by verified ID.
Host-side: the CLI writes only its runtime directory (journal, lock, marker, scratch); it never touches the app's files.

## 3. Abort conditions — stop at once, change nothing further, export the journal
Any of: `doctor` or `plan` not exit 0 · `status` shows anything unresolved · inventory files cannot be written · before-inventory shows a container/image/volume already carrying a Harpoon label for this runtime that the journal does not know · Harpoon log shows `HOST_FORWARD_DISCOVERY_FAILED` / `HOST_FORWARD_TRY_FALLBACK` · `lsof` shows a listener that is not exactly `127.0.0.1:<port>` owned by the Harpoon pid · a health response without this container's marker · the port mapping differs from the lease · guest free space unverifiable, too small, or the management channel and Docker disagree about which guest this is (hostname or kernel) · build over budget · any Docker error that is not the one being tested · any journal error · **any difference in the after-inventory other than the expected ones** · the user says stop.

## 4. Ownership-verified cleanup (only what the journal says Harpoon created)
- Failed or interrupted run: `recover`, then `reconcile` as a **dry run**, read every line, then `reconcile --apply --authorize-live-docker-changes=…` only with the user's go-ahead.
- **Successful run:** `harpoon-deploy teardown --runtime-dir R --deployment dep_… --docker-socket S --resources-dir RES` is a **dry run** that prints exactly what would happen. With `--apply --authorize-live-docker-changes=I-UNDERSTAND-THIS-CHANGES-REAL-DOCKER-STATE` it
  1. refuses unless the deployment is in the journal and `succeeded`, nothing about it was refused earlier, and every recorded container/image has a receipt;
  2. lists the runtime (an unlistable runtime stops everything with exit 5 and **no journal write**);
  3. one container at a time, in the order they were journaled — **the first that does not come out clean (a failed removal or a refusal) stops the rest**, which are reported as not attempted and get no journal record; located by its recorded ID, with its name and **all three ownership labels** matching; a mismatch (labels differ, or the name is held by a different ID) is **recorded as refused and never touched**; absent is recorded as absent; otherwise it is journaled (attempt), stopped, removed with `force=false&v=false`, and the result journaled;
  4. only if every container is verified gone: re-lists, and removes the image **by ID** (`force=false&noprune=true`) unless any container still references that image ID or its user cannot be identified (then the image is kept and said so);
  5. only if every container is verified gone **and** Docker no longer maps the lease's port: releases the lease (a ledger-only attempt + result).
  A failure at any step is recorded as a failed cleanup, later steps are not attempted, the exit code is 7, and a repeat run retries only what is left; a repeat after success prints "nothing to do". It never prunes, never forces, never matches by name pattern, never adopts.
- Never: removal by name pattern, tag, `prune`, `-f`, or any object the journal does not list.

## 5. Authorization points (each separate, each recorded)
A. Read-only access to the live Docker socket and management socket: `doctor`, `plan`, `inventory`.  B. The deployment itself (builds one image, starts two containers).  C. Teardown of what B created.  D. The LAN-isolation test from a second device.

## 6. Verification that must be shown, not asserted
Image ID in the journal equals the ID Docker reports for the container's image · the three labels on the image and containers · marker in the response equals the marker in the container's environment · `NetworkSettings.Ports` equals the lease before and after · single loopback listener owned by the Harpoon pid · before/after inventories differ only as listed · the LAN test result for D · the real `/containers/create` and inspect responses captured for the fake.

## 7. Simulated evidence versus what only the real runtime can show
| Claim | Shown offline (how) | Needs the real runtime |
|---|---|---|
| Image identity: random tag, intent first, ID by `--iidfile`, checked by ID + labels, containers from the ID, moved tag refused | `b7_readiness.rs` with a fake Docker and a fake Buildx that writes the ID file | Real Buildx's `--iidfile` content (ID vs index digest under the containerd image store); real `--load` tagging |
| Marker flow end to end | `b7_fixture.rs`: the **real fixture script** under `/bin/sh` behind loopback listeners, real HTTP, environment taken from the container-create request | That `nc -lk -p 8080 -e /srv.sh` runs in `alpine:3.22`; the container actually receives its environment; the forwarder carries the header unchanged |
| Port mapping before/after | Fake inspect output shaped like Docker's documented `NetworkSettings.Ports` | Real inspect output |
| Host listener is Harpoon's, loopback-only | Parser/verifier on recorded-format `lsof -F` text and fake `lsof` scripts | Real `lsof` output; the app's real pid/process name; any second listener |
| Guest free space | Fake guest speaking the management protocol; identity cross-check, `df` parsing, fail-closed on every malformed shape (`b7_guest.rs`) | Real `hostname`/`uname -r`/`df -B1` output in the guest; that busybox `df` prints one line for the data root; that Docker's `DockerRootDir` is on the filesystem Harpoon grows |
| Same guest on both channels | hostname and kernel equal | Two guests with identical hostname and kernel would be indistinguishable — not excluded |
| Teardown | Fake Docker: success, partial failure, mismatched ownership, absent, unreachable, repeated, image in use, mapped port | Real stop/remove semantics, real 409 conflicts |
| Source snapshot | Real files in temp directories, swaps injected at the worst moment | Behaviour with the real project and real `.dockerignore` rules |
| Shipping CLI refuses live deployment | In-process and real-binary tests: exit 6 before reading, writing or contacting anything | — |
| LAN isolation, forwarder lag, fallback guest IP, real Docker responses | **Not shown at all** | Everything |
