# B7 Phase 2A — read-only live preflight

Date (UTC): 2026-10-31 ~07:20–07:40. Repository: `main` at `89b609c` (includes #115). Authorization: read-only inspection of the running Harpoon through its Docker and management
sockets (Phase 2A only). **Nothing was built, created, started, stopped, removed, pruned, restarted, repaired or modified.** Raw sanitized evidence is kept outside the repository and outside every
Harpoon runtime/journal directory, at `…/scratchpad/b7-preflight/` of the working session (paths below are relative to it). A before/after inventory is identical (`cli/53` vs `cli/54`), the proposed B7 runtime directory
(`~/.harpoon-b7-runtime`) was never created, and the Harpoon process (pid 67874) was not restarted.

Method, in full: Docker — `GET` only (`collect_get.py`, 22 requests, all GET, log in `docker-api/29-request-log.txt`; plus the merged CLI's `doctor`/`plan`/`inventory` and `deploy-docker/examples/live_preflight.rs`, which
call only Harpoon's own GET methods). Guest — exactly `hostname`, `uname -r`, `df -B1 /var/lib/docker` through the management socket (`mgmt/30-raw-replies.txt`). Host — `ps`, `lsof` (listening sockets and one pid's descriptors), `stat`, reads of
Harpoon's `harpoon.log` and `runtime.json`. No Docker CLI, no context, no `DOCKER_HOST`.

**Legend:** PASS observed directly · FAIL observed incompatibility · UNKNOWN cannot be established without a mutating operation · BLOCKED a required capability is unavailable.

## Summary
| # | Finding | Status |
|---|---|---|
| F1 | Docker Engine 28.3.3, API 1.51 (min 1.24), overlay2 on ext4 (not the containerd store) | PASS |
| F2 | Harpoon's Docker parsers accept every real answer (`examples/live_preflight`: 30 PASS, 0 FAIL) | PASS |
| F3 | Management channel: real reply format matches; `MgmtDfProbe` accepts it; 31 099 641 856 bytes free | PASS |
| F4 | Hostname/kernel agreement between Docker and the channel | PASS, **weak** (hostname is the unset default `(none)`) |
| F5 | `alpine:3.22` present locally | PASS |
| F6 | Running forwarder: pid 67874, process name **`harpoon`** (not `Harpoon`); owns the Docker, management and control sockets | PASS, **default `--forwarder-command Harpoon` would FAIL** |
| F7 | Guest-IP discovery succeeded (`192.168.64.3`); fallback IP never used; no collisions | PASS |
| F8 | Real `lsof -Fpcn` format parses (it adds `f<fd>` lines, which the parser tolerates) | PASS |
| F9 | Loopback binding of a *published* port on the real host | UNKNOWN (no published port exists; history shows `127.0.0.1:<port> -> 192.168.64.3:<port>`) |
| F10 | Bundled Buildx in the installed `Harpoon.app` | **FAIL** (absent) |
| F11 | A pin-verified Buildx v0.38.1 exists in a development build directory | PASS (provenance), not yet in a `Resources/harpoon/bin` layout |
| F12 | `--iidfile`, `--load` against the real engine | UNKNOWN |
| F13 | Running-container inspect shape, create/start/stop/remove, `nc -lk -e` in alpine, marker delivery, forwarder lag, LAN isolation | UNKNOWN |
| F14 | Shipping CLI live gate | closed (by design) — Phase 2B needs a decision |

## 1. Baseline
| | Command / source | Result | Interpretation |
|---|---|---|---|
| Revision | `git rev-parse HEAD` in the preflight worktree | `89b609c25bd1701faaa678209de8a0b9f79d4ad2` (merge of #115) | The code under test. The user's main checkout is at an old commit (`4503cde`) and was not touched. |
| `harpoon-deploy` | `which harpoon-deploy`; built `deploy-cli` release from `main` | none installed; built binary sha256 `5eae4663c61e013c…` | The CLI used for this report is a local build of `main`. |
| Harpoon process | `ps`; `~/Library/Application Support/Harpoon/runtime.{pid,json}` | pid **67874**, started 2026-10-29 23:47:57 local, `…/Harpoon/harpoon/build/harpoon run`, 2 CPUs, 4096 MiB, uuid `F54DA9AB-…` | The running Harpoon is a **development build (binary dated Oct 17) from the user's primary checkout**, not the installed `Harpoon.app` (0.1.1, Sep 5). It was not restarted. Whether it equals the source I audited in B7 phase 1 is UNKNOWN; the log lines match the audited behaviour (F7, F9). |
| Sockets | `stat` | `/tmp/harpoon-docker.sock`, `/tmp/harpoon-mgmt.sock`, `/tmp/harpoon-control`: type socket, mode `0600`, owner `juancastro`; `lsof -p 67874 -U` shows pid 67874 holding all three | PASS. Nothing guessed or created. |
| State | `harpoon.log` (live; a `HARPOON_PORT_SYNC_RESULT` every 10 s up to the time of writing) | `containers=9 mappings=0` | The forwarder is alive and polling; nothing is published. |
| Stray process of mine | `ps` | a leftover `b5_journal` **test binary** (pid 62406, ~3 h old, from an earlier session, unrelated to Harpoon) | Harmless; not touched here. Tell me if you want it killed. |

## 2. Docker API (GET only)
| | Request | Status | Interpretation |
|---|---|---|---|
| Ping | `GET /_ping` | 200 `OK`; `Api-Version: 1.51`, `Builder-Version: 2`, `Docker-Experimental: false`, `Swarm: inactive` | `Builder-Version: 2` = BuildKit is the daemon's default builder. |
| Version | `GET /version` | Docker/28.3.3, API 1.51, MinAPI 1.24, linux/arm64, kernel `6.12.110-0-virt`; containerd v2.1.5, runc 1.3.4 | Harpoon's unversioned paths are fine on this daemon. |
| Info | `GET /info` | `Name="(none)"`, `DockerRootDir=/var/lib/docker`, `KernelVersion=6.12.110-0-virt`, `Driver=overlay2`, backing fs `extfs`, `CgroupDriver=cgroupfs`, 9 containers (0 running), 6 images | **overlay2 classic image store**: after `--load`, the image ID in `inspect` is the config digest, which is what `--iidfile` is documented to write (UNKNOWN until a build — F12). |
| Containers | `GET /containers/json?all=1`, 9× `GET /containers/{id}/json` | all `exited`; `Ports: []`; inspect has `NetworkSettings.Ports={}` and `HostConfig.PortBindings={}` | Parsers accept (F2). |
| Images | `GET /images/json`, 6× `GET /images/{id}/json`, `GET /images/alpine:3.22/json` | 200; `Config.Labels: null`, `Config.Volumes: null` (postgres:16 declares `/var/lib/postgresql/data`) | Inspect **by ID and by tag both work**; `null` labels/volumes are handled. |
| Volumes | `GET /volumes` | 1 anonymous local volume (`com.docker.volume.anonymous`) | The inventory records it; deployments never touch volumes. |

**Real-versus-fake discrepancies (all benign for the parsers, recorded for the fake's fidelity):**
1. `/info` `Name` is `"(none)"`, the fake's default is `"harpoon-guest"`.
2. `df` header is `1-blocks` (busybox), the fake prints `1B-blocks`; the device is `/dev/vda`, mount `/`.
3. Image inspect `Config.Labels` and `Config.Volumes` are `null`; the fake emits `{}`/null variously.
4. Container inspect `HostConfig.PortBindings` is `{}` for no bindings; the fake emits `null`.
5. Real inspect/list objects carry many more fields (`State.*`, `Mounts`, `GraphDriver`, `NetworkSettings.Networks`, …) that the fake omits.
6. The management reply has spaces after `:` and `,` (`{"exit": 0, "stdout": "…", "stderr": ""}`); the fake's compact form parses identically.
7. **Not comparable (UNKNOWN):** the `NetworkSettings.Ports` of a *running published* container (the fake says `HostIp` `0.0.0.0` and `::`), `POST` create/start/stop and `DELETE` semantics and error bodies, and the `ImageID` of containers created from an image ID.

## 3. Guest management channel (exactly three commands)
| argv | Reply (verbatim, sanitized: none needed) | Exit | Interpretation |
|---|---|---|---|
| `hostname` | `{"exit": 0, "stdout": "(none)\n", "stderr": ""}` | 0 | equals Docker `Name` |
| `uname -r` | `{"exit": 0, "stdout": "6.12.110-0-virt\n", "stderr": ""}` | 0 | equals Docker `KernelVersion` |
| `df -B1 /var/lib/docker` | `{"exit": 0, "stdout": "Filesystem           1-blocks       Used Available Use% Mounted on\n/dev/vda             33741029376 1228779520 31099641856   4% /\n", "stderr": ""}` | 0 | two lines, six fields, mount `/` contains the data root, total 33.7 GB, used 1.23 GB, **available 31 099 641 856 bytes** |

Each reply is exactly one newline-terminated JSON line then EOF, as the client requires. `MgmtDfProbe::measure()` accepted all three (F3: "31099641856 bytes free on (none) (6.12.110-0-virt), filesystem holding /var/lib/docker"); the CLI `doctor` reports
"31099 MB free … 2147 MB required". **Identity (F4):** matching hostname and kernel is supporting evidence only — `(none)` is what any Linux with no hostname set reports, and the kernel string is shared by every VM from the same guest image. The probe's identity
check therefore adds little on this guest; the real binding is that both sockets are held by the same Harpoon pid (F6).

## 4. Host forwarding
| | Source | Result | Interpretation / next action |
|---|---|---|---|
| Process identity | `ps`, `lsof -p 67874 -Fpcft` | pid 67874, **command name `harpoon`** | `LsofCheck` compares the name (case-sensitive, 9-char truncation). The CLI default is `Harpoon` → the listener check would **fail** on a correct listener. Pass `--forwarder-command harpoon` (no code change needed) and the pid current at 2B time. |
| Listeners | `lsof -nP -iTCP -sTCP:LISTEN -Fpcn` (exit 0, 870 bytes) | system listeners only (ControlCenter, rapportd, …); **none in 38000–39099; Harpoon holds no TCP listener** | Nothing is published; ports 38000 and 39000 are free on the host and unmapped in Docker. |
| `lsof -F` format | same | `p<pid>` / `c<cmd>` / `f<fd>` / `n<addr>` lines | The parser ignores `f`; accepted. Whether the real address of the forwarder's listener prints as `127.0.0.1:<port>` is UNKNOWN until one exists. |
| Guest IP | log: `HARPOON_GUEST_IP_DISCOVERED 192.168.64.3` (2026-10-30T04:48:14Z) | counts: `HOST_FORWARD_DISCOVERY_FAILED` 0, `HOST_FORWARD_TRY_FALLBACK` 0 | The hard-coded fallback was **not** used. |
| Collisions / errors | log | `HARPOON_PORT_FORWARD_COLLISION` 0, `HOST_FORWARD_ACCEPT_ERROR` 0 | None. |
| Historical forwarding | log (2026-10-30 07:26–07:50, benchmark containers `hb-pm-*`) | 44 `ADD`/`LISTENING`/`REMOVE` events, all `127.0.0.1:<p> -> 192.168.64.3:<p>/tcp containerPort=8080`; 289 `HOST_FORWARD_CONNECT_FAILED … Connection refused` | Confirms loopback binding, same-number mapping and the guest IP **historically**. The refusals are the expected window while a container starts (and the 10 s sync lag): health probes must tolerate them (they do: 60 s deadline). |
| Cadence | log | sync every 10 s, currently `containers=9 mappings=0` | Matches the audit. |

LAN isolation is **not** established (separately authorized test required).

## 5. CLI preflight (merged `main`, explicit sockets, proposed future project `deploy-docker/fixtures/http-marker`)
| Command | Exit | Output (essential) | Interpretation |
|---|---|---|---|
| `status --runtime-dir ~/.harpoon-b7-runtime` | 0 | "journal: not initialised (nothing was created)" | No journal; nothing created. |
| `doctor … --health-path /health --container-port 8080 --resources-dir /Applications/Harpoon.app/Contents/Resources --docker-socket … --mgmt-socket …` | **3** | `[ok]` source (2685 bytes, base `alpine:3.22`), `[FAIL]` bundled Buildx missing, `[ok]` journal location, `[ok]` runtime ping, `[ok]` base image present, `[ok]` guest free space (31099 MB, 2147 MB required); "5 ok, 0 warnings, 1 failures" | The only failure is the Buildx (F10). Everything else the CLI can check live is green. |
| `plan` (same source, Docker socket) | 0 | five operations; test port **39000**, application port **38000**; "blockers: none"; "live execution: disabled in this build" | Ports free in Docker and on the host. |
| `inventory --lsof-path /usr/sbin/lsof` | 0 | 9 containers, 6 images, 1 volume, no mappings, no host listeners in range; run twice, **identical** | The before-inventory for Phase 2B (`cli/53`); no change between runs. |

## 6. Buildx and base-image readiness
| | Observation | Status |
|---|---|---|
| Installed app | `/Applications/Harpoon.app/Contents/Resources/harpoon/bin/` contains only `harpoon`; no `docker-buildx` | **FAIL** — `doctor` exit 3 |
| Development artifact | `Harpoon-deploy-b2/harpoon/build/docker-buildx`: universal (1 arch) Mach-O, `codesign --verify --strict` OK, Authority Docker Inc (9BNSXJN65R), hardened runtime; `docker-buildx version` (empty HOME, no daemon) = **v0.38.1**; its arm64 slice (`lipo -thin`) has sha256 **6f20ba24…3012 = the pin**, and the cached download slice has the same hash | PASS (provenance). The whole-file hash differs from the pin only because the fetch script `lipo`-wraps the verified slice. |
| Capabilities | `docker-buildx build --help` lists `--iidfile`, `--load`, `--label`, `--pull`, `--progress`, `--output` | present in the CLI; **not** proven against the engine (F12) |
| Layout | the CLI needs `<Resources>/harpoon/bin/docker-buildx` (symlinks are refused) | no such tree exists yet |
| Base image | `alpine:3.22` id `sha256:2c15e55d…`, arm64, labels null, no volumes | PASS |
| Daemon builder | `Builder-Version: 2` | BuildKit is available to Buildx's docker driver (UNKNOWN until a build) |

## 7. Existing workloads that must be preserved
9 containers, all `exited` (code 255 ×7, 0 ×1; killed when the VM last started — they have no restart policy) : `harpoon-ui-stack-test-20261015-{a-worker,b-api,b-worker,c-api,c-cache,c-worker,c-exited}-1`, `harpoon-ui-preview-{alpha,beta}-1`. None is Harpoon-deployment-owned (`io.harpoon.managed` absent); none publishes a port.
6 images: `alpine:3.22`, `alpine:latest`, `postgres:16`, `hello-world:latest`, and two leftover `stage8-reclamation-*:latest`. 1 anonymous volume. The Phase 2B before/after `inventory` must show exactly the expected additions (one image, one running container) and nothing else.

## 8. Remaining prerequisites for the first disposable deployment
1. **Buildx tree (local, non-live):** assemble a private Resources directory with `harpoon/bin/docker-buildx` from the verified artifact (copy, not symlink) and re-run `doctor` until it exits 0.
2. **A gate-open binary (decision needed):** the shipping `Gate` is closed on purpose. Phase 2B needs a separately built, clearly named binary or feature that opens it, reviewed on its own — the shipping CLI must stay closed. I did not do this.
3. **Flags for 2B (from this report):** `--docker-socket /tmp/harpoon-docker.sock --mgmt-socket /tmp/harpoon-mgmt.sock --forwarder-pid <current pid, now 67874> --forwarder-command harpoon --lsof-path /usr/sbin/lsof --health-path /health --container-port 8080 --resources-dir <prepared tree>`. Re-read the pid immediately before (it changes on restart).
4. **Unknowns that only the authorized live steps can close:** F12–F13. Abort conditions in `DEPLOY_B7_LIVE_ACCEPTANCE.md` apply unchanged.
5. **Weak identity (F4):** acceptable because both sockets belong to one Harpoon pid, but note it in the 2B record.
6. **Decide about the stray test process (pid 62406)** and whether the running development build of Harpoon is the one to test against, given that the installed app is a different (older) build with no Buildx.

## Recommendation
**Runtime compatibility: GO.** Docker, the management channel, the guest disk probe, the base image, the forwarder and its logs are all compatible with what B1–B7 expect, and nothing existing is at risk beyond what the abort conditions cover.
**Execution readiness for Phase 2B: NO-GO right now** — not because of the runtime, but because two local prerequisites are open (a verified Buildx tree for `--resources-dir`, and an approved way to run a gate-open binary) and one flag must differ from the default (`--forwarder-command harpoon`). Close those offline (no live access needed except a final `doctor` re-run), then Phase 2B is a **GO** on your explicit authorization.
