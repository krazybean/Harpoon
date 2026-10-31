# B7 live acceptance checklist (Phase 2 — requires the user's separate authorization)

Nothing in this document has been run. Phase 1 (this PR) is offline only. **No step below may be performed without the user's explicit, separate authorization for that step.**

## 0. Preconditions — all must hold before step 1
- [ ] Written authorization from the user naming: the Harpoon runtime/socket, the disposable application (`deploy-docker/fixtures/http-marker`), and each authorization point in section 5.
- [ ] A reviewed change opens `Gate::shipping()` for live deployment (it is a constant; nothing else opens it).
- [ ] **Phase 2 must first provide, and test offline:** (a) a real guest free-space probe (until then a live deploy stops at its first step: "no guest-side free-space probe is implemented"); (b) an explicit, journaled, ownership-verified **teardown** of a *succeeded* deployment's containers and image — today `reconcile` removes only what a *failed or interrupted* deployment should not have kept, so there is **no supported way to remove the serving container of a successful disposable deployment**; (c) captured real Docker API responses to replace the documentation-derived fake.
- [ ] `cargo fmt/clippy/test` green for `deploy`, `deploy-docker`, `deploy-journal`, `deploy-cli` on the exact commit used.
- [ ] A fresh runtime directory (`0700`) used for nothing else; `runtime-kind` = docker.

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
**Read-only:** `GET /_ping`; `GET /containers/json?all=true`; `GET /containers/{id}/json`; `GET /images/json`; `GET /images/{ref}/json` (base image, tag, ID); `GET /volumes` (inventory only); `GET /containers/{id}/logs` (on failure); `lsof` (fixed arguments).
**Changing Docker state (and nothing else):**
1. One build through the **bundled** Buildx (`--load`, `--pull=false`, three ownership labels, `--iidfile`, unique tag `harpoon/<app>:<dep>-<128 random bits>`), from a private snapshot of the project folder. Creates one image.
2. `POST /containers/create` ×2 — `hp-<dep>-pre` (test port 39xxx) and `hp-<dep>` (application port 38xxx): created **from the verified image ID**, labels ×3, `HARPOON_DEPLOYMENT_MARKER=<random per container>`, 256 MiB / 0.5 CPU / 64 PIDs, no swap, no restart policy, no `HostIp`, no binds, mounts, volumes, privileged, host network or devices.
3. `POST /containers/{id}/start` ×2.
4. `POST /containers/{id}/stop` then `DELETE /containers/{id}?force=false&v=false` for the preliminary container, after re-inspecting id, name and all three labels.
5. [Phase 2 to build] the guest free-space measurement and the teardown of section 4, each specified and tested before use.
No pull, prune, exec, network, volume-changing or image-removal call is made except by the teardown/reconcile procedures below, each by verified ID.
Host-side: the CLI writes only its runtime directory (journal, lock, marker, scratch); it never touches the app's files.

## 3. Abort conditions — stop at once, change nothing further, export the journal
Any of: `doctor` or `plan` not exit 0 · `status` shows anything unresolved · inventory files cannot be written · before-inventory shows a container/image/volume already carrying a Harpoon label for this runtime that the journal does not know · Harpoon log shows `HOST_FORWARD_DISCOVERY_FAILED` / `HOST_FORWARD_TRY_FALLBACK` · `lsof` shows a listener that is not exactly `127.0.0.1:<port>` owned by the Harpoon pid · a health response without this container's marker · the port mapping differs from the lease · guest free space unverifiable or too small · build over budget · any Docker error that is not the one being tested · any journal error · **any difference in the after-inventory other than the expected ones** · the user says stop.

## 4. Ownership-verified cleanup (only what the journal says Harpoon created)
- Failed or interrupted run: `recover`, then `reconcile` as a **dry run**, read every line, then `reconcile --apply --authorize-live-docker-changes=…` only with the user's go-ahead. Each removal re-checks id, name and all three labels at that moment and is by ID.
- Successful run: **teardown command to be built and tested in Phase 2** (journal intent → verified stop/remove of the serving container by ID with `force=false&v=false` → verified image removal by ID with `noprune=true`). Until it exists, the disposable container stays; the user decides.
- Never: removal by name pattern, tag, `prune`, `-f`, or any object the journal does not list.

## 5. Authorization points (each separate, each recorded)
A. Read-only access to the live socket: `doctor`, `plan`, `inventory`.  B. The deployment itself (builds one image, starts two containers).  C. Teardown of what B created.  D. The LAN-isolation test from a second device.

## 6. Verification that must be shown, not asserted
Image ID in the journal equals the ID Docker reports for the container's image · the three labels on the image and containers · marker in the response equals the marker in the container's environment · `NetworkSettings.Ports` equals the lease before and after · single loopback listener owned by the Harpoon pid · before/after inventories differ only as listed · the LAN test result for D · the real `/containers/create` and inspect responses captured for the fake.
