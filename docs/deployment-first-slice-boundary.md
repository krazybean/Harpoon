# Milestone B — the smallest real deployment boundary

> Design and isolation rules for the first slice. Step status (B1–B8.x and what follows) is tracked only in
> [the roadmap](roadmap.md#deployment-control-plane-harpoon-21--canonical-milestone-tracker); the plan table in §6 is the
> original plan, not a status.

Target slice: **local folder → Dockerfile build → container start → health check → logs → truthful result.**
Builds on `docs/deployment-control-plane-requirements.md` (D1–D10), the offline engine foundation (`deploy/`, PR #106)
and the findings in `docs/results/DEPLOY_BUILDX_VALIDATION.md` and `COMPAT_BUILDX_AND_PORT_FORWARDING.md`.

**Out of scope for this slice:** providers, Git/polling/webhooks, previews, Kubernetes, cutover of a *previous* release
(first deployment only), rollback/promote/resume, secrets/Keychain, SQLite (an append-only JSON-lines journal is enough
for slice 1), the shipping UI. The UI stays on the mock until the slice is proven.

## Approved decisions (2026-10-30)
1. **JSON-lines journal** — approved *exclusively* for the first disposable deployment slice. Requirements: durable writes
   (`write` + `fsync` of the file, and of the directory on creation), tolerant recovery of a **partial trailing record**
   (truncate-or-skip, never panic, never misread), **resource intent and receipt records** (see B1), and crash-injection
   tests. **SQLite remains the approved permanent metadata store and must replace this journal before any real release
   replacement or rollback.**
2. **Local base image only.** The first live test uses an image already present in Harpoon. No image pulls and no external
   Dockerfile frontends (`# syntax=`). Guest DNS troubleshooting (gate G1) stays a separate task.
3. **Port ranges:** applications **38000–38999**, disposable tests **39000–39099**. Before assigning a port check (a) real
   host listeners on `127.0.0.1`, (b) existing Docker port mappings, (c) journaled leases.
4. **First-test limits (initial test defaults, not universal application limits):** memory 256 MiB, 0.5 CPU, 64 PIDs, no
   restart policy, build context ≤ 256 MB, startup health deadline 60 s, build deadline 10 min. A **disk-space preflight**
   runs before the build. The engine **never prunes** anything automatically.
5. **Port publishing** without `HostIp` (`-p PORT:CPORT`). Harpoon's host listener must stay loopback-only and the live test
   must verify LAN isolation. Rewriting Docker API create requests is deferred.

### Limitations that follow from the decisions
- Publishing without `HostIp` binds the port on the guest NAT address as well as guest loopback; the host side is loopback-only,
  but another local process on the Mac can reach the guest NAT address (compat doc B3). Documented, not hidden.
- A user's own `-p 127.0.0.1:P:C` still fails through the forwarder; this slice does not change that.
- JSON-lines gives no multi-record atomicity or indexed queries; it must not outlive the first slice.
- Local-base-only means a Dockerfile whose `FROM` image is absent fails at build; that failure is reported truthfully, not worked around.
- The limits above are conservative test values and say nothing about what real applications need.

## 0. Principles for this slice
- **Host-side only.** The engine runs in the host process and talks to Docker through Harpoon's own socket
  (`/tmp/harpoon-docker.sock`) with the bundled `docker-buildx` and `docker` CLI. No Docker Desktop, no guest changes.
- **Record before acting.** Every external side effect is preceded by a journal write that names the resource
  (foundation guarantee, extended by the resource ledger, review gap #1).
- **Truthful result.** `succeeded` only if the final health check observed what the evidence says it observed;
  otherwise `failed` with the failing step and the log tail. "Local health ≠ production compatibility" is stored as
  evidence text, never as a pass/fail.
- **Everything is labelled** `io.harpoon.managed=true`, `io.harpoon.app=<app_…>`, `io.harpoon.deployment=<dep_…>`
  (label keys already in `deploy/src/ids.rs`); the engine only ever touches objects carrying its own labels.

## 1. Host-side operations
| # | Operation | Implementation | Isolation rule |
|---|---|---|---|
| 1 | **Select folder** | UI/CLI passes an absolute path; engine canonicalises it (`realpath`), requires a directory with a `Dockerfile` (or a given relative path inside), rejects symlinks that escape the folder, honours `.dockerignore`. No copy: the context is streamed by buildx. | Read-only access to the folder. Never writes into it. Maximum context size guard (default 256 MB) with a clear failure. |
| 2 | **Build image** | `docker-buildx build --load --progress=plain -t harpoon/<app>:<dep> --label io.harpoon.*` with an explicit env (`DOCKER_HOST`, empty `HOME`/`DOCKER_CONFIG` under Harpoon's dir). No secrets in slice 1. No `# syntax=`/external frontends (gate G1). | Image tag namespace `harpoon/…` only. Build is **not cancellable** mid-way (vsock/BuildKit): cancel = stop waiting, discard, clean up. Step deadline (default 10 min). Disk-space preflight first; never prunes. |
| 3 | **Create container** | `POST /containers/create` via the Docker API (not a shell string): name `hp-<app>-<dep>`, labels, `HostConfig.PortBindings` = `{<cport>/tcp: [{HostPort: "<lease>"}]}` **without `HostIp`** (compat doc B3/B5), restart policy `no`, no bind mounts, no privileged, `Memory`=256 MiB, `NanoCpus`=0.5 CPU, `PidsLimit`=64 (first-test defaults). | Creates only containers named/labelled for this deployment; never reuses or modifies an existing container or volume. |
| 4 | **Start on a loopback port** | `POST /containers/<id>/start`. Port lease from a configured range (default 38000–38999), chosen by probing `127.0.0.1` on the host **and** the existing mappings from `GET /containers/json`; recorded in the journal before create. Host exposure is loopback-only by construction (forwarder binds `127.0.0.1:<port>`); the guest NAT binding is documented, not hidden. | Lease released by cleanup. Port collision ⇒ pick the next free port once, then fail. |
| 5 | **Health check** | Host-side probe of `http://127.0.0.1:<port><path>` (default `GET /`; configurable path, expected status class), polling every 500 ms until success or `startup_deadline` (default 60 s), with `N` consecutive successes (default 2). TCP-connect fallback only when the app declares no HTTP port. Container must also still be `running` (not restarting/exited). | The probe never follows redirects off-host. Evidence stored (see §2). |
| 6 | **Capture logs** | Build: the buildx plain-progress stream. Runtime: `GET /containers/<id>/logs?stdout&stderr&timestamps` (bounded tail, default last 2 000 lines / 1 MB, captured at result time and on demand). Both go through the redaction sink; stored as files next to the journal, referenced from the step record. | Size caps with an explicit "truncated" marker. Redaction best-effort, no length floor (spec D8). |
| 7 | **Record result** | Engine drives the existing state machine (`Queued→Running→Succeeded/Failed/Cancelled/Interrupted`), steps `source, build, preliminary_check, cutover (start), final_check`. First deployment: no previous release ⇒ no recovery, no pause. | A crash at any point leaves a journal that `recover_on_start` turns into `interrupted` plus a ledger of resources to reconcile. |

## 2. Data contracts (Rust; the offline crate gains these before any I/O)
```rust
pub struct FolderSource { pub root: PathBuf, pub dockerfile: PathBuf, pub context_bytes: u64 }
pub struct BuildSpec    { pub image_tag: String, pub labels: Vec<(String,String)>, pub deadline_ms: u64 }
pub struct PortLease    { pub host_port: u16, pub container_port: u16 }           // persisted before use
pub struct ContainerSpec{ pub name: String, pub image: String, pub lease: PortLease,
                          pub labels: Vec<(String,String)>, pub memory_bytes: u64, pub cpus: f32 }
pub struct HealthSpec   { pub path: String, pub expect: StatusClass, pub interval_ms: u64,
                          pub consecutive_ok: u8, pub startup_deadline_ms: u64 }
pub struct CheckEvidence{ pub kind: CheckKind, pub target: String, pub observed: String,
                          pub attempts: u32, pub elapsed_ms: u64, pub scope_note: &'static str }
                          // scope_note is always: "local container only — not a guarantee of production behaviour"
pub enum  Resource      { Image{tag,id}, Container{name,id}, Port(PortLease) }     // the ledger entries
pub struct LogRef       { pub path: PathBuf, pub bytes: u64, pub truncated: bool, pub redactions: u32 }
pub struct Outcome      { pub status: DeploymentStatus, pub failure: Option<Failure>,
                          pub evidence: Vec<CheckEvidence>, pub logs: Vec<LogRef>, pub resources: Vec<Resource> }
```
Trait changes (offline, with the fake updated and every existing test kept): `Backend` steps take a `&mut dyn LogSink`
and a deadline, return structured evidence for checks, and `Journal` gains `record_resource`. `Backend` implementations:
`FakeBackend` (tests) and `DockerBackend` (slice 1), the latter the only place that spawns processes or opens sockets.

## 3. Failure semantics (what "truthful" means)
- Build fails ⇒ `failed`, `build`, log tail; nothing created, no cleanup needed beyond the ledger check.
- Container fails to create/start ⇒ `failed`, `cutover`; created container removed (ledger), image kept (labelled, never served).
- Health never succeeds within the deadline ⇒ `failed`, `final_check`; container stopped **and** removed, last runtime log
  tail attached, evidence lists the attempts. The deployment is **not** "succeeded but unhealthy".
- Container exits during the check ⇒ `failed` with the exit code.
- Cleanup itself fails ⇒ recorded as a separate field in the outcome; the deployment is still `failed`, and the leftover
  resource stays in the ledger and in the UI as "needs attention" (never silently ignored).
- Engine/host crash ⇒ `interrupted` on next start; ledger reconciled by label (stop/remove only objects with this
  deployment's label, and only after listing them).

## 4. Isolation requirements (apply to the first disposable test)
- **Disposable workload:** a tiny HTTP server image already **present locally** (gate G1) — the test folder is generated
  under a fresh temp directory with a `Dockerfile` of at most `FROM <local base>` + `COPY` + `CMD`. Dedicated app id
  `app_test…`, port range reserved for tests (e.g. 39000–39099), names prefixed `hp-test-`.
- **Never** touch containers/volumes not carrying the test label; never `prune`, never `docker rm -f` by pattern; cleanup
  removes only the exact ids in the ledger.
- Do not stop/restart the VM, change VM resources, alter the Docker socket bridge or any guest/release artifact.
- The test runs against the already-running Harpoon; if the runtime is not running, the test **skips**, it does not start it.
- Record before/after counts of images, containers, volumes and the BuildKit cache; the test fails if anything unlabelled changed.
- Requires **explicit written authorization** for the test workload before execution (this document does not grant it).

## 5. Stop conditions (abort the slice and report)
1. Any unlabelled container, volume or image is created, changed or removed.
2. The VM, socket bridge or forwarder state changes (restart, reconnect) because of an engine action.
3. A port other than `127.0.0.1:<lease>` becomes reachable, or the lease collides with a non-test mapping.
4. The health check reports success without the evidence fields populated, or `succeeded` is recorded while the container is not running.
5. A secret-shaped value appears in a log file, journal entry or test output.
6. Base-image resolution requires the network (G1) and the cause is not yet understood.
7. Cleanup cannot account for every ledger resource.

## 6. Minimal implementation plan
| Step | Deliverable | Needs VM |
|---|---|---|
| B1 | Close review gaps #1–#5 offline: resource ledger, per-step deadlines, `LogSink` + redaction, `PortLease`, `CheckEvidence`; extend fake + tests; crash-at-every-write incl. ledger | no |
| B2 | `harpoon/fetch-buildx.sh`, bundle + signing + verify scripts (compat doc A6) and `tools/verify-buildx-build.sh` | built app; VM for the test |
| B3 | `DockerBackend`: Docker API client over the Unix socket (create/start/inspect/logs/stop/rm, list for port occupancy) + buildx runner; contract tests against a **recorded-response fake server** (no VM) | no |
| B4 | Folder validation, port lease allocator, health prober with an in-process test HTTP server | no |
| B5 | Journal as JSON-lines files + log files; `recover_on_start` reconciliation by label | no |
| B6 | CLI-only driver (`harpoon deploy --folder …`) behind a feature flag; **no UI wiring** | no |
| B7 | First disposable deployment against the live VM, §4 isolation, with explicit authorization | **yes** |

Decisions 1–5 above were approved; B1 may start. B2 onward needs its own go-ahead, B7 explicit written authorization.
