# Underbelly Foundation — Orchestration Log

This log records the Stage 0–1 execution, including failed attempts, retries, replans, and explicit stop decisions. It is intentionally empirical: an attempted check is not recorded as a pass merely because the repository contains historical evidence for it.

| Sequence | Action | Result | Decision |
|---:|---|---|---|
| 1 | Read the referenced “Continue Harpoon Maintenance” conversation | Confirmed prior plan: observe first; preserve Docker ownership; execute Stages 0–6 before refactoring; keep an empirical log | Use as context only; current checkout remains authoritative |
| 2 | Checked the ChatGPT project mirror | Mirror contained only `AGENTS.md` and an empty `sources/`; no Git checkout | Locate the actual checkout instead of inventing a baseline |
| 3 | Checked the verified local Harpoon checkout | Clean `main`, `HEAD=642d91d`, tracking `origin/main` | Use this checkout as the working repository |
| 4 | Inspected available artifacts/worktrees | No attached managed worktree existed | Work in the verified local checkout with explicit approval for outside-mirror access |
| 5 | Started three parallel read-only reviews | Inventory, documentation drift, and runtime anatomy reviews launched | Keep write scope local to the parent; no agent edits accepted |
| 6 | Read current Swift, guest init, management, builder, acceptance, and architecture sources | Found current `/tmp` sockets, Swift foreground VM owner, `socat`/Python management, and stale `harpoond`/`~/.harpoon` docs | Record observed-vs-inferred distinctions in the canonical anatomy |
| 7 | Ran `sh tools/validate.sh` | Exit 2: repository gate requires Node 20, host has Node v22.23.3 | Record as `BLOCKED`, do not downgrade or change toolchain files |
| 8 | Considered running live VM/Docker acceptance | Would require a behavior-bearing environment run and is not needed to produce Stages 0–1; historical evidence is qualified | Stop at baseline/documentation scope; mark live acceptance `NOT RUN` |
| 9 | Wrote Stage 0 baseline, Stage 1 anatomy, and this log | Documentation-only change; no source, asset, harness, or config behavior edited | Verify diff and working-tree scope before handoff |
| 10 | Formalized the Stage 0–1 exit barrier | Added `tools/stage-0-1-gate.sh` and `docs/underbelly/stage-0-1-exit-gate.md`; the barrier reuses `tools/validate.sh`, selects Node 20 explicitly, and adds the requested live socket/Docker/management checks | Run the gate; Stage 2 remains prohibited until it returns `STAGE_0_1_GATE_PASS` |
| 11 | Ran the formal Stage 0–1 exit gate | `/tmp/harpoon-stage-0-1-gate-final.log`: Node `v20.20.2`; `HARPOON_VALIDATION_PASS`; VM running; Docker ready; actual `/tmp/harpoon-docker.sock`; actual `/tmp/harpoon-mgmt.sock`; Docker API and `alpine:3.22 true`; management `exec`; bounded PTY management shell; final `STAGE_0_1_GATE_PASS` | Stage 0–1 is formally closable; do not begin Stage 2 in this change |

| 12 | Read-only parallel Stage 2 reviews | Reused M15 pinned Harpoon/VM identity rules; confirmed disk-backed VZ association is indirect; selected deterministic Alpine idle/CPU/memory workloads; rejected the heavier M9 Compose fixture | Keep attribution domains separate and preserve explicit unavailable/ambiguous semantics |
| 13 | Implemented tools/stage-2-resource-gate.sh | Added repeatable A/B/C collection for host ps/vmmap, guest proc and smaps_rollup, process trees, Docker identity, cgroup v2 files, and lifecycle probes | No production source or runtime stack behavior changed |
| 14 | First live Stage 2 attempt | Collector stopped on guest BusyBox ps state incompatibility; no valid verdict; later inspection found the gate's own stale named container | Use guest-supported stat, clean only owned names after Docker readiness, and retry |
| 15 | Second live Stage 2 attempt | STAGE_2_GATE_PASS with A/B/C evidence, three shims observed, runc observed, and explicit exec/stop samples; durable reports written | Add VZ disk-owner association and PSS provenance before final rerun |
| 16 | Final live Stage 2 gate | HARPOON_STAGE2_SAMPLES=10 bash tools/stage-2-resource-gate.sh returned STAGE_2_GATE_PASS; State A 0, B 1, C 2 running containers; VZ service association observed; PSS/cgroup provenance present | Stage 2 is closed; do not begin Stage 3 or broader refactors |

## Retries and non-events

- The first repository inspection was attempted in the project mirror and failed because it is not a Git repository. This was a location error, not a repository failure.
- The existing validation gate was run once. No retry was made because the failure is deterministic for the current Node version and retrying cannot change it.
- No live VM start, Docker workload, image pull, root image mutation, package install, or acceptance harness was run.
- No planned Rust guest service, daemon rename, socket-path migration, or shell-to-compiled rewrite was started.
- The final barrier run was live on the permitting host and passed. Its report is preserved at `/tmp/harpoon-stage-0-1-gate-final.log`; repeatable runs should set `HARPOON_GATE_REPORT` to preserve a report elsewhere.

## Replan / stop decision

Stage 0–1 and Stage 2 are complete in this scoped change. Stage 3, boot audit,
host audit, and any production refactor remain explicitly out of scope.

| 17 | Started Stage 3 from the Stage 2 PASS precondition | Initial project mirror was empty; the actual checkout was located and existing Stage 0–2 working-tree artifacts were preserved | Audit the actual checkout; do not invent a new baseline or overwrite prior evidence |
| 18 | Read-only boot trace and source correlation | Traced `tools/guest-builder/src/init` lines 1–944, builder/release closure checks, management wrapper/handler, and `VMManager` serial readiness polling | Split initramfs and post-`switch_root`; classify mixed phases instead of forcing one label |
| 19 | First live Stage 0–1 attempt | Sandbox blocked Swift linker output and production Application Support state; gate returned FAIL for environment permissions, not product behavior | Retry once with host permissions; do not change runtime paths |
| 20 | Host-permitted Stage 0–1 live run | Node `v20.20.2`, `HARPOON_VALIDATION_PASS`, VM running, Docker API/run, management exec and PTY shell all passed; final `STAGE_0_1_GATE_PASS` | Capture fresh serial and host log evidence before closing Stage 3 |
| 21 | Fresh serial boot capture | Observed block mount, runtime/module refresh, `HARPOON_PIVOT`, root validation, disk no-resize, networking, `/run` tmpfs, Docker readiness, vsock 2375, `HARPOON_NET_READY`, management attempt 1 and `HARPOON_MGMT_READY`; host logs supplied timestamps only for Docker and management | Record deterministic order; mark guest per-phase elapsed time unobservable |
| 22 | Stage 3 artifacts added | Added `docs/underbelly/stage-3-guest-boot-audit.md`, `docs/underbelly/stage-3-guest-boot-phases.json`, and `tools/stage-3-guest-boot-gate.sh`; updated runtime anatomy with live evidence; no production source changed | Run syntax/map checks, then rerun Stage 0–1 and Stage 2 |
| 23 | Reran Stage 2 after Stage 3 additions | `HARPOON_STAGE2_SAMPLES=10 HARPOON_STAGE2_SAMPLE_DELAY=0.1 bash tools/stage-2-resource-gate.sh` returned `STAGE_2_GATE_PASS`; durable attribution report refreshed | Preserve Stage 2 PASS and do not alter Docker/guest runtime |
| 24 | Ran complete Stage 3 barrier | `HARPOON_GATE_LIVE=auto HARPOON_STAGE2_SAMPLES=10 bash tools/stage-3-guest-boot-gate.sh` reran Stage 0–1 and Stage 2; Stage 0–1 log ended `STAGE_0_1_GATE_PASS`, Stage 2 log ended `STAGE_2_GATE_PASS`; artifact checks passed | Stage 3 is formally closable |
| 25 | Deterministic Stage 3 artifact barrier confirmation | `HARPOON_STAGE3_SKIP_PRIOR_GATES=1 bash tools/stage-3-guest-boot-gate.sh` returned `STAGE_3_GATE_PASS` after the live prior-gate rerun | Explicitly stop at Stage 3; do not start Stage 4 or UI work |
| 26 | Ownership-map correction and final full rerun | Added explicit builder/host/guest-init/guest-runtime ownership domains; final `HARPOON_GATE_LIVE=auto HARPOON_STAGE2_SAMPLES=10 bash tools/stage-3-guest-boot-gate.sh` completed with Stage 0–1 and Stage 2 logs ending in PASS; deterministic artifact check returned `STAGE_3_GATE_PASS` | Formally close Stage 3 and stop |

## Final stop decision

Stage 0–1, Stage 2, and Stage 3 are complete in this scoped change. Stage 4,
host audit, Harpoon 2 UI work, and any production refactor remain explicitly
out of scope.

| 27 | Started Stage 4 host-runtime audit from Stage 0–1, Stage 2, and Stage 3 PASS preconditions | Project mirror again contained no checkout; the actual checkout was verified at `642d91d` with prior working-tree artifacts preserved | Audit only; do not edit `sources/`, Swift production behavior, Docker Engine/containerd/runc, Stage 5, or UI |
| 28 | Attempted local QMD historical retrieval | Repository-local `.qmd/index.sqlite` returned `SQLITE_CANTOPEN` under the available sandbox | Record the limitation; rely on current source and existing gate evidence, do not invent history |
| 29 | Parallel read-only runtime reviews plus parent source trace | Mapped Lifecycle, VMManager, BridgeSet, Unix/Mgmt/Balloon bridges, port reconciliation, CLI process model, timers, queues, blocking calls, and recovery paths | Preserve current boundaries; classify duplicated/implicit authority rather than refactor |
| 30 | Added Stage 4 audit, JSON ownership map, and gate | Documentation and gate only; no production source changed | Run prior barriers and Stage 4 barrier; stop at Stage 4 |
| 31 | Static Stage 4 barrier and shell syntax/diff checks | `bash -n`, `git diff --check`, and `HARPOON_STAGE4_SKIP_PRIOR_GATES=1 bash tools/stage-4-host-runtime-gate.sh` returned `STAGE_4_GATE_PASS` | Proceed to host-permitted live precondition rerun |
| 32 | First full Stage 4 attempt in restricted sandbox | Swift linker could not write `harpoon/build/harpoon`; Application Support was not writable; Stage 0–1 and Stage 2 failed for environment access, while Stage 3 artifact check passed | Request the same host-level permission used for prior live gates; do not alter runtime paths |
| 33 | Host-permitted Stage 0–1 rerun | Node `v20.20.2`, `HARPOON_VALIDATION_PASS`, VM/Docker/management/shell smoke passed; sockets observed as `srw-------`; final `STAGE_0_1_GATE_PASS` | Preserve live evidence and continue to Stage 2 |
| 34 | Host-permitted Stage 2 rerun | `HARPOON_STAGE2_SAMPLES=10 HARPOON_STAGE2_SAMPLE_DELAY=0.1 bash tools/stage-2-resource-gate.sh` returned `STAGE_2_GATE_PASS` | Preserve Stage 2 PASS |
| 35 | Deterministic Stage 3 artifact barrier | `HARPOON_STAGE3_SKIP_PRIOR_GATES=1 bash tools/stage-3-guest-boot-gate.sh` returned `STAGE_3_GATE_PASS` | Stage 3 remains PASS; do not begin Stage 5 or UI |
| 36 | Final Stage 4 barrier | `HARPOON_STAGE4_SKIP_PRIOR_GATES=1 bash tools/stage-4-host-runtime-gate.sh` returned `STAGE_4_GATE_PASS`; full prior barriers were independently rerun immediately before this artifact barrier | Formally close Stage 4 and stop |
| 37 | Full Stage 4 barrier from the normal path | `HARPOON_GATE_LIVE=auto HARPOON_STAGE2_SAMPLES=10 HARPOON_STAGE2_SAMPLE_DELAY=0.1 ./tools/stage-4-host-runtime-gate.sh` returned `PASS Stage 0-1 gate`, `PASS Stage 2 gate`, `PASS Stage 3 gate`, and `STAGE_4_GATE_PASS` | Formal Stage 4 close confirmed; stop before Stage 5/UI |

## Stage 4 final stop decision

Stage 4 is formally closed: `STAGE_4_GATE_PASS`. No Stage 5, UI work, daemon
implementation, or production behavior refactor is authorized by this audit.

| 38 | Started Stage 5 Bash/tooling audit from the Stage 0–4 PASS precondition | The attached project mirror was empty; the actual checkout was located and existing working-tree artifacts were preserved | Audit only; no production, guest image, Docker Engine/containerd/runc, Swift, or UI changes |
| 39 | Read-only parallel reviews plus parent inventory | Counted 63 extension-based shell/Python files (9,392 LOC) and three extensionless guest runtime files (1,279 LOC); traced guest init, Python management, socat listeners, builder/release, gates, regressions, milestone scripts, and host/Tauri shell-outs | Classify mixed responsibilities by section; do not force `src/init` or `harpoon-mgmt` into a tooling-only label |
| 40 | Security and duplication pass | Found manifest parsing/repair, `pkill -f` socat cleanup, unowned background socat listeners, interpolated `bash -c` release checks, unquoted Docker-ID expansions, and duplicated build/runtime closure checks | Record findings without fixing them; preserve current implementation and stack |
| 41 | Added Stage 5 audit, inventory JSON, and tooling gate | Documentation, machine-readable classification, syntax/static checks, and prior-barrier wrapper only; no runtime source changed | Run deterministic barrier first, then host-permitted prior barriers if available |
| 42 | Deterministic Stage 5 barrier | `HARPOON_STAGE5_SKIP_PRIOR_GATES=1 bash tools/stage-5-tooling-gate.sh` returned `STAGE_5_GATE_PASS`; inventory/schema, all script syntax, and diff checks passed | Continue to the required host-permitted prior-barrier rerun |
| 43 | First normal Stage 5 barrier attempt in restricted sandbox | Stage 0–1 could not link `harpoon/build/harpoon` or write Application Support; Stage 2 could not start Harpoon; Stage 3 artifact checks passed | Record as environment failure; do not claim a Stage 5 pass or change runtime paths |
| 44 | Host-permitted Stage 5 barrier rerun | `HARPOON_GATE_LIVE=auto HARPOON_STAGE2_SAMPLES=10 HARPOON_STAGE2_SAMPLE_DELAY=0.1 bash tools/stage-5-tooling-gate.sh` returned prior-barrier PASS and `STAGE_5_GATE_PASS`; Node `v20.20.2` path was used by the existing validation gate | Formally close Stage 5; stop before Stage 6/UI/refactor |

## Stage 5 final stop decision

Stage 5 is audit-complete and stops at the tooling boundary. The current
Python `harpoon-mgmt` plus `socat` implementation remains acceptable for
current production behavior but is a transitional guest-runtime boundary.
Future Rust `harpoon-guest` work is deferred; no Stage 6, UI work, daemon
implementation, or production refactor is authorized by this audit.

| 45 | Started Stage 6 Docker Boundary Audit from the Stage 0–5 PASS precondition | Actual checkout verified; project mirror was empty; existing dirty artifacts preserved | Audit Docker ownership only; no refactor, UI work, or stack change |
| 46 | Two parallel read-only evidence passes plus parent source trace | Mapped Docker socket/API transport, CLI/UI invocations, context setup, PortForwardManager, host-path translation, guest dockerd/readiness, volumes, networks, Compose/BuildKit, logs/exec/shell, and resource attribution | Record Docker as authority and classify boundary state separately from Docker metadata |
| 47 | Static Stage 6 artifacts added | Added audit, machine-readable boundary matrix, gate, and narrow runtime-anatomy invariant; no production source changed | Run syntax, schema, diff, prior barriers, and one owned Docker lifecycle probe |
| 48 | Source challenge pass | Found stale guest-IP/duplicate-port forwarding keys, duplicated Swift/Tauri context setup including a foreign-context fallback, UI error-to-zero counts, and a UI config-path projection mismatch | Record as `DUPLICATED_SEMANTICS_REVIEW`; do not fix during Stage 6 |
| 49 | Stage 6 barrier | Prior Stage 0–5 gates, static matrix checks, and the owned Docker create → start → inspect → logs → exec → stop → rm path passed | Close Stage 6; do not begin Stage 7 or UI work |

## Stage 6 final stop decision

Stage 6 is formally closed: `STAGE_6_GATE_PASS`. Docker Engine/containerd/runc,
Linux networking/storage semantics, and production behavior were preserved.
Stage 7 diagnostics, UI work, and Stage 9 cleanup are explicitly not started.

| 50 | Started Stage 7 Diagnostics Contract from the Stage 0–6 PASS precondition | Used two parallel read-only inspections plus parent trace; preserved the dirty checkout | Implement only diagnostics; no Stage 8, Stage 9, UI, daemon, guest-agent rewrite, or Docker authority change |
| 51 | Reused existing runtime seams | `statusSnapshot`, `runDocker`, the existing management socket, `/proc`, and cgroup v2 evidence were sufficient; no new control plane was justified | Keep host, guest, and workload domains separate and bounded |
| 52 | Implemented snapshot, explain, and bounded subscribe polling | Added the Swift collector, bounded management capture, CLI/build integration, schema fixture, contract document, and Stage 7 gate | `subscribe` emits snapshots only; lifecycle/container event types are deferred because no truthful durable event source exists |
| 53 | Static Stage 7 validation | Fixture, required markers, shell syntax, whitespace, Swift build/signing, and stopped-runtime snapshot checks passed; lifecycle and Docker semantics were unchanged | Run the host-permitted prior barriers; stop at Stage 7 |
| 54 | Full Stage 7 barrier attempt | Stage 0–1, Stage 3, Stage 4 static, Stage 6 owned Docker probe, and Stage 7 checks passed; Stage 2 refused the live baseline because pre-existing container `1b25b78151c0` was present | `BLOCKED_HOST_STATE`; do not delete or mutate an unowned container; no formal Stage 7 close claimed |

## Stage 7 final stop decision

Stage 7 implementation is complete and its deterministic barrier passes. Formal
closure is pending a clean host rerun of the pre-existing Stage 2 baseline;
Stage 8 performance, Stage 9 cleanup, and Harpoon 2 UI work were not started.

| 55 | Clarified Stage 2 verdict semantics and ownership safety | Updated the Stage 2 gate and dependent wrappers so `PASS`, `FAIL`, and `BLOCKED_ENVIRONMENT` are distinct; exit codes are 0, 1, and 2 respectively. State A remains a strict zero-container baseline; only containers labeled by this harness are cleanup-eligible. | Preserve non-pass environment blocks through Stages 3–7 without relabeling them as product failures |
| 56 | Inspected live Docker state before cleanup | `docker --context harpoon ps -a --no-trunc` failed: `unix:///tmp/harpoon-docker.sock` does not exist. No container list or ownership evidence was available. | `BLOCKED_ENVIRONMENT`; no cleanup, stop, image, volume, network, or user-state mutation performed |
| 57 | Attempted formal Stage 0–7 rerun | Not run past the Stage 2 precondition because Docker was unreachable; Stage 7 deterministic/static implementation checks remain separate from full-chain closure. | Stage 7 is not formally closed; exact external blocker is the missing/unreachable Harpoon Docker socket |

## Validation follow-up — 2026-10-14

Exact commands run:

```sh
docker --context harpoon ps -a --no-trunc
bash -n tools/stage-2-resource-gate.sh tools/stage-3-guest-boot-gate.sh \
  tools/stage-4-host-runtime-gate.sh tools/stage-5-tooling-gate.sh \
  tools/stage-6-docker-boundary-gate.sh tools/stage-7-diagnostics-gate.sh
HARPOON_STAGE7_SKIP_PRIOR_GATES=1 bash tools/stage-7-diagnostics-gate.sh
```

The Docker inspection returned exit 1 because `/tmp/harpoon-docker.sock` was
absent. Cleanup was not safe to attempt and was not performed. The full chain
therefore remains `BLOCKED_ENVIRONMENT`; no Stage 8, Stage 9, or UI work began.

## Durable stage-closure rule

From this recovery onward, a stage is not formally closed when its gate merely
passes. Closure requires the stage changes to be committed, pushed to the
remote, and the exact commit SHA recorded in the stage report and orchestration
log. A later Work/session boundary must therefore leave a durable branch and
provenance, not only a successful local working tree.
