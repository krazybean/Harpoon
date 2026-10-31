# B7 — first live disposable deployment: results

**B7 first-deployment acceptance: PASS.**
This is acceptance of *one* disposable deployment, built, served, isolated and torn down on the real Harpoon runtime under the controls described below. It is **not** a statement of general production readiness; see "Remaining unverified behaviour".

Date: 2026-10-31 (UTC). Everything in this report was observed directly on the runtime named below, or is marked as unverified. This document was written afterwards from the captured evidence; no test was run to write it, and nothing was contacted.
Historical records are unchanged and still describe what was planned and known at the time: [`DEPLOY_B7_LIVE_ACCEPTANCE.md`](DEPLOY_B7_LIVE_ACCEPTANCE.md), [`DEPLOY_B7_FORWARDING_AUDIT.md`](DEPLOY_B7_FORWARDING_AUDIT.md), [`DEPLOY_B7_LIVE_PREFLIGHT.md`](DEPLOY_B7_LIVE_PREFLIGHT.md), [`DEPLOY_B7_PHASE2B_PLAN.md`](DEPLOY_B7_PHASE2B_PLAN.md).

## 1. What was tested
| | |
|---|---|
| Host | macOS on Apple silicon |
| Running Harpoon | a **development build** of `harpoon run` (binary dated 2026-10-17, sha256 `33259856f6161ee4…`), pid **67874**, started 2026-10-29 23:47:57 local. Not the installed `Harpoon.app` 0.1.1, which has no bundled Buildx. |
| Guest | Linux kernel `6.12.110-0-virt`; hostname unset (`(none)`); 2 CPUs, `MemTotal` 4 109 737 984 bytes; `DockerRootDir=/var/lib/docker`; 31 099 641 856 bytes free there before the deployment (via `df -B1`, management channel) |
| Docker | Engine **28.3.3**, API **1.51** (minimum 1.24), `overlay2` on `extfs` (the classic image store), BuildKit is the daemon's default builder (`Builder-Version: 2`), containerd v2.1.5, runc 1.3.4 |
| Buildx | **v0.38.1**, the pinned artifact from B2: arm64 slice sha256 `6f20ba2437865311ef99015bf5d1039c3c8e4d2177c1c64aa41b134b5ecd3012` (equals the pin, the signed checksum line and the cached download); whole-file sha256 `70dbccab5e0d261353a802315651f6827cee90bb22f5f3eeb39ffc7bbe55e075` (the verified slice wrapped by `lipo`); Developer ID Docker Inc (9BNSXJN65R), hardened runtime; `codesign --verify --strict` valid. Used from a private, byte-identical copy at `~/.harpoon-b7-prep/Resources/harpoon/bin/docker-buildx` (0700 directories, regular file). |
| Deployment CLI | the **experimental** `harpoon-deploy-b7-experiment`, sha256 `23905f40404abafd9b77e60e8330321710f4ae20746eccba11d0a08aeeaed702`, build revision `236ccece6f0e417039c74211d2f2d9886f8af921` — `main` `89b609c` plus one local-only commit that opens the live-deployment gate. Used for the single `deploy` only. |
| All other commands | the **shipping** `harpoon-deploy` (gate closed), built from `main` sources identical to `a63684e` for the CLI, engine, backend and journal crates (sha256 `5eae4663c61e013c…`): `status`, `doctor`, `plan`, `inventory`, `export-journal`, `teardown` |
| Fixture | `deploy-docker/fixtures/http-marker` (Dockerfile sha256 `c16347a2…565e`, `srv.sh` `fd798e0c…c03d`), `alpine:3.22` base, busybox `nc -lk -p 8080 -e /srv.sh` |

## 2. Baseline that had to survive
Captured read-only before anything changed (Phase 2A) and re-captured before the deployment (identical): **9 containers** (all `exited`, none Harpoon-owned: `harpoon-ui-stack-test-20261015-*`, `harpoon-ui-preview-{alpha,beta}-1`), **6 images** (`alpine:3.22`, `alpine:latest`, `postgres:16`, `hello-world:latest`, two `stage8-reclamation-*:latest`), **1 anonymous volume**, no published ports, no host listeners in 38000–39099. Phase 2A found the runtime compatible: every real Docker response was accepted by Harpoon's own parsers (32 checks, 0 failures); the management channel returned `hostname`/`uname -r`/`df -B1` in the expected format and `MgmtDfProbe` accepted it (31 099 641 856 bytes free); the forwarder had discovered the guest IP (`192.168.64.3`) and never used the fallback.

## 3. The deployment (single attempt, no retry)
- 2026-10-31 08:25:47Z: forwarder pid rediscovered (67874, owns the Docker socket, lsof name exactly `harpoon`); 08:25:53Z: `deploy` returned **exit 0**. One invocation.
- Deployment `dep_01M68H17PKXJWKYX7MGYG74GPH`, application `app_01M68H17PJG833TTK7MY22E0XH`. Steps `source`, `build`, `preliminary_check`, `cutover`, `final_check` all **Succeeded**; `recovery` skipped. Evidence: preliminary check Healthy (4 attempts, 2/2 consecutive, HTTP 200); final check Healthy (3 attempts, 2/2, HTTP 200); both from real HTTP (`simulated=false`).
- **Image identity (verified against the real engine):** the image ID Buildx wrote through `--iidfile`, the journal receipt, Docker's `GET /images/{id}/json` `Id`, the container's `Image`, and the container list's `ImageID` are all `sha256:329e6e0e78c5e7bbda5a06e447ed825940d59cad77fb831ca167ddfc50af8c95`. The tag was `harpoon/app_01m68h17pjg833ttk7my22e0xh:dep_01M68H17PKXJWKYX7MGYG74GPH-10401cdac8c9e4dd012351adba186bd9` (128 random bits). The image carried exactly the three ownership labels, declared no volumes, linux/arm64, 8 540 103 bytes.
- **Container** `hp-dep_01M68H17PKXJWKYX7MGYG74GPH` (id `aaf066b6a431da55c891b340f469d2edccceb9959567c911544aee6fb5b11272`): created from the image **ID**; running; three labels; 256 MiB, swap disabled (`MemorySwap=-1`), 0.5 CPU, 64 PIDs, restart policy `no`, not privileged, bridge network, no binds or mounts; the only environment besides `PATH` was `HARPOON_DEPLOYMENT_MARKER`.
- **Docker's real shapes:** `NetworkSettings.Ports` for the running container was `{"8080/tcp": [{"HostIp":"0.0.0.0","HostPort":"38000"},{"HostIp":"::","HostPort":"38000"}]}`, `HostConfig.PortBindings` `{"8080/tcp":[{"HostIp":"","HostPort":"38000"}]}`, and the container list shows the port twice (`0.0.0.0` and `::`) — as the fake assumed.
- **Before/after inventory differed by exactly four lines:** the new container, `docker-port-mapping 38000`, `host-listener 127.0.0.1:38000 pid=67874 command=harpoon`, and the new image. Nothing else changed.

## 4. HTTP health and the per-container marker
`GET http://127.0.0.1:38000/health` through the live forwarder: **HTTP 200**, header `X-Harpoon-Deployment-Marker` equal to the container's own `HARPOON_DEPLOYMENT_MARKER` (32 hex characters, never journaled, not given to Buildx), body equal to the marker. `GET /` returned **404** without the marker. The same result was reproduced during the LAN test and before teardown. The listener was a single `127.0.0.1:38000` owned by pid 67874 (`harpoon`); the Harpoon log shows `127.0.0.1:39000 -> 192.168.64.3:39000` for the preliminary container, its removal, then `127.0.0.1:38000 -> 192.168.64.3:38000`, with no collision, no fallback IP, and no connection-refused events.

## 5. LAN isolation (IPv4 and IPv6) — PASS
Mac: Wi-Fi `en0`, `192.168.1.109/24`, gateway `192.168.1.1`, macOS application firewall **disabled**, default route on `en0` (no VPN in the path). Second device: a **Windows laptop** on the same Wi-Fi, `192.168.1.229`. The Mac's global IPv6 addresses (eight, one `/64`; prefix omitted here) were all tested.
- **Connectivity control (laptop):** `Test-NetConnection` — `PingSucceeded=True`, RTT 9 ms, `TcpTestSucceeded=False`.
- **IPv4:** `curl http://192.168.1.109:38000/health` → connection refused, exit 7, no HTTP response.
- **IPv6:** the same against each of the eight addresses → connection refused, exit 7, no HTTP response.
- **Passive packet capture on the Mac** (`en0`, non-promiscuous, headers only, no payload retained; filter: TCP to port 38000 and ICMP echo requests addressed to the Mac). It shows the laptop's packets **reaching the Mac's LAN interface**: 4 ICMP echo requests from `192.168.1.229`; **10 SYNs** from `192.168.1.229` to `192.168.1.109:38000` (two attempts × 5 retransmissions); and **5 SYNs from the laptop's IPv6 address to each of the Mac's eight IPv6 addresses** (40 in total). **No packet other than a SYN was ever addressed to port 38000**, i.e. no connection was established. (The filter was destination-only, so the Mac's refusals are not in the capture; they are the laptop's own report.) The capture also recorded an unrelated device, `192.168.1.246`, sending the Mac an ICMP echo request about once a minute.
- **After the remote tests:** localhost `/health` 200 with the correct marker; the only listener still `127.0.0.1:38000`, pid 67874; no connection from a non-loopback peer; the Docker inventory identical to the baseline taken before the test; no forwarder add/remove events.
- Meaning: the host side of the published port is loopback-only and is unreachable from another device on the same Wi-Fi over IPv4 and IPv6. This is the design (the forwarder maps every Docker bind address to `127.0.0.1`), now measured.

## 6. Teardown of `dep_01M68H17PKXJWKYX7MGYG74GPH` — PASS
- **Preflight (read-only, all passed):** pid 67874 owns the Docker socket; deployment `succeeded` in the journal; container running with the recorded ID, name, image ID and all three labels; image found by its recorded ID with all three labels and the journaled tag; **only this deployment's container referenced the image**; exactly one container carried this deployment's labels and no other Harpoon-managed container existed; localhost `/health` 200 with the marker; listener `127.0.0.1:38000` owned by Harpoon; 10 containers / 7 images / 1 volume, identical to the saved post-deployment inventory.
- **Dry run** (shipping CLI): proposed exactly the recorded container (`aaf066b6…1272`), the recorded image (`sha256:329e6e0e…8c95`) "only once its containers are gone and no other container uses it", and the 38000 lease; no foreign target. It changed nothing.
- **Apply, one attempt, 2026-10-31 08:47:11Z: exit 0, "teardown: complete".** No force, no prune, no name-pattern removal, no retry, no `reconcile`.
- **Result:** the container and the image return **404** by ID; no host listener on 38000 (curl: connection refused); the forwarder logged `…REMOVE … 127.0.0.1:38000`; the lease 38000 is released.
- **Journal:** **five cleanup attempts and five results, all `removed`, zero unresolved**: port 39000 and the preliminary container (released during the deployment's own cutover), then — in the teardown — the serving container, the image, and port 38000. Totals in the exported journal: 1 header, 10 snapshots, 5 intents, 5 receipts, 5 attempts, 5 results; `status` reports "outstanding leases: none, unresolved cleanup: none"; the deployment stays recorded as `succeeded`. A second dry run reports "nothing to do — every recorded resource … is already released" (exit 0). Journal and lock files are mode 0600 in 0700 directories.
- **Final inventory vs the original Phase 2A baseline: byte-identical** — the nine original containers (same IDs, names, states), the six original images, the one volume. 9 containers, 6 images, 1 volume, no mappings, no listeners.
- **Harpoon was not restarted:** pid 67874 throughout, uptime continuous (1 day 3 h 59 min at the end).

## 7. Things the earlier plan listed as unknown, now observed
| Was unknown | Observed |
|---|---|
| `--iidfile` / `--load` against the real engine | ID written by Buildx equals Docker's image `Id`; the tag named that image (overlay2 classic store) |
| `FROM alpine:3.22` with `--pull=false` and no registry | resolved from the local store; the build succeeded |
| Buildx first run with an empty scratch `HOME` and `DOCKER_HOST=unix://…` | worked (the scratch directory now holds Buildx's own state, inside the runtime directory) |
| `nc -lk -p 8080 -e /srv.sh` in `alpine:3.22` | works; `/health` answers with the marker, other paths 404 |
| Marker delivered through the container environment and carried by the forwarder | yes, header and body |
| `NetworkSettings.Ports` of a running published container; real `lsof -F` output | as above; `lsof` printed `p67874 / charpoon / f10 / n127.0.0.1:38000` |
| Teardown against the real engine | stop, remove by ID, remove image by ID, lease release all succeeded |

## 8. Remaining unverified behaviour (not covered by this acceptance)
- **Live failure paths:** none was exercised. A failed build, a refused ownership check, an unhealthy container, a journal failure mid-deployment, and **interrupted-deployment recovery** (`recover`, then `reconcile --apply`) have been tested only against the fakes.
- **Guest private-NAT exposure:** the guest's own port 38000 on the VM's private network (`bridge100`, `192.168.64.0/24`) was not tested from another host. It is reachable from the Mac by design; whether anything else on that segment can reach it is unmeasured.
- **Intermittent forwarder startup delays:** the forwarder polls every 10 s, and a ~10 s lag was predicted and appeared historically (289 connection-refused events in an earlier benchmark). This run's first probe succeeded immediately, so the behaviour is **not ruled out**; it only did not occur once.
- **Production release and packaging readiness:** the tested Harpoon was a development build, not the signed/installed app; the installed `Harpoon.app` 0.1.1 has **no bundled Buildx**; the bundled-Buildx packaging, signing and a release `Resources` layout were not exercised end to end; the experimental binary was a local, unmerged build.
- Also unverified: more than one concurrent deployment; a second deployment of the same application (release replacement is not implemented); a larger or multi-stage build; images that declare volumes (rejected by design, not tried live); a guest with a real hostname (here the hostname is `(none)`, so the hostname cross-check is weak and the real binding of the two channels is that one Harpoon pid holds both sockets); other interfaces than the tested Wi-Fi; Intel hosts.

## 9. What stays as it was
- The **shipping deployment gate is closed**: `Gate::shipping()` is unchanged on `main`, and the ordinary binary still refuses live `deploy` (exit 6).
- The **experimental gate-opening code is not merged**: it exists only as one commit (`236ccece6f0e417039c74211d2f2d9886f8af921`) on the local branch `experiment/b7-live-gate`, never pushed.
- No Docker resource, journal or runtime state was changed to write this report.

## 10. Evidence
Raw captures were kept locally, outside the repository and outside every Harpoon runtime and journal directory, and sanitized (container environments, labels of other workloads and secrets redacted; the IPv6 prefix omitted). They are not committed. Index: Phase 2A (`b7-preflight/`: Docker GET structures, management-channel replies, parser results, lsof output, CLI `doctor`/`plan`/`inventory`); Phase 2B (`b7-phase2b/`: preflight identity, before/after inventories, deployment stdout/exit, status, Docker observations, marker and forwarder evidence, journal export); LAN test (`b7-lan/`: interfaces, capture filter and capture, analysis, re-checks); Phase 2C (`b7-teardown/`: preflight, dry runs, apply output and exit, final inventory, status, journal export).

## Verdict
**B7 first-deployment acceptance: PASS** — one disposable deployment, verified image identity, per-container marker verified over the real forwarder, LAN isolation measured over IPv4 and IPv6 with packet-level confirmation, ownership-verified journaled teardown, every pre-existing resource and the Harpoon process preserved.
**General production readiness: not claimed.**
