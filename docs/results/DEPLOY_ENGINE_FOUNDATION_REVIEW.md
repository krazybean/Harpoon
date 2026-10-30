# Deploy engine foundation — review before real I/O

Scope: `deploy/` (crate `harpoon-deploy`, no dependencies), rebased onto `main` at `3d07e58`.
Result of the review of state transitions, cutover journal, crash recovery and the application queue.
Nothing here adds I/O; the only code change in this review is one clippy fix (`io::Error::other`).

## What was verified
- `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test --locked --offline`: 47 tests
  (26 unit + 21 lifecycle) pass on Rust 1.97.1. New workflow `deploy-engine.yml` runs the same, path-filtered,
  plus a guard that the crate declares no dependencies and uses no `std::net`/`std::process`/async/db APIs.
- **State transitions:** the 8 × 8 transition relation is table-tested. Terminal states have no exits;
  every non-terminal state may become `interrupted`. Verified by reading `state.rs` against the tests.
- **Cutover journal:** `begin_cutover` is persisted *before* `stop_current`; a trace test checks that every
  side effect is preceded by a write describing it. Only a failure after the cutover began triggers the one
  bounded restoration; ordinary failures never stop, restore or pause anything.
- **Crash recovery:** a crash at every journal write of five scenarios recovers to a consistent state;
  `recover_on_start` is idempotent, marks running steps `interrupted`, flags incomplete cutovers.
- **Queue:** one running deployment per application, newest queued supersedes older queued (reported, never
  dropped), the running one is never interrupted, `finish` of a non-running id is ignored.

## Findings: gaps to close *before or with* the first real I/O
Ordered by how likely each is to cause a false "success" or an unrecorded side effect.

| # | Gap | Why it matters with real I/O | Proposed handling |
|---|-----|------------------------------|-------------------|
| 1 | **No resource ledger.** A temporary preliminary-check container/port, a half-started `start_new`, or a failed `stop_new` (its result is discarded in `fail_after_cutover` and the first-deployment path) leaves no record. | Orphaned containers/ports after a failure or crash; "restored" could be reported while a stray container still holds the address. | Journal every created resource (kind, id, labels) *before* creating it; add a `cleanup` step with its own recorded outcome; `recover_on_start` lists unreleased resources. Cleanup failure surfaces in the report, never ignored. |
| 2 | **No per-step deadlines.** Only the 60 s restoration budget exists. | A hung build, `docker start` or health probe blocks the application queue forever (the vsock channel has no cancel). | `Policy` gains per-step budgets; the backend must honour them; timeout = failed step with kind `Timeout`. Vsock-buffered builds cannot be cancelled mid-way: the plan treats cancel as "stop waiting, discard result, clean up". |
| 3 | **No log contract.** `Backend` returns only `Result<(), StepError>`. | Build/runtime logs, the truthful failure message and redaction cannot be captured. | A `LogSink` passed to each step (bounded ring + spill file), redaction applied at the sink (spec D8, no length floor). `StepError.message` must come from the sink tail, not free text. |
| 4 | **No port allocation or host-address contract.** | Needs `127.0.0.1:<port>` allocation, collision handling, and a verified loopback-only bind (see compat doc: `-p 127.0.0.1:P:C` currently fails through the forwarder). | `PortLease` in `Context`, persisted before use; released by cleanup. |
| 5 | **Health check is a boolean.** `preliminary_check`/`final_check` return `Ok(())`. | Cannot record *what* was checked (HTTP path, status, attempts, duration) — needed to keep "local health ≠ production compatibility" honest. | Return a `CheckEvidence { kind, target, observed, attempts, elapsed }`, stored on the step. |
| 6 | **Journal durability is unspecified.** `persist(&Deployment)` is one whole-record write. | SQLite must make each persist atomic and durable (WAL + `synchronous=FULL` for cutover/recovery writes). | Specify in the persistence PR; add a crash-injection test against the real journal. |
| 7 | **In-flight state is memory-only after restart.** `recover_on_start` marks queued/running deployments `interrupted` rather than re-queuing. | Acceptable and truthful (spec: interrupted, user retries) but must be explicit in the UI contract. | Keep; document. No auto-retry. |
| 8 | **Release semantics.** A release record exists once the build succeeds even if the deployment later fails (tested, "built but never served"). | The Docker image must be tagged/labelled so "retained image" ≠ "ever served"; cleanup of never-served images must not touch served ones. | Add `served: bool` (or derive from the cutover) before the retention/GC work; GC is out of scope for slice 1. |
| 9 | **First-deployment final-check failure** stops the new container best-effort and records nothing about that stop. | Same as #1. | Covered by the ledger. |
| 10 | **Cancel** is honoured only before the cutover; a running build cannot be interrupted. | Truthful, but the UI's "Cancel" must say "will stop after the build". | Document; no change. |

None of these invalidates the approved architecture. #1, #2, #3 and #5 change trait signatures (`Backend`, `Journal`)
and should land first, still offline, before any Docker call is written.

## Safety guarantees preserved
No test was changed or removed. The crate still has no dependencies, no network/process/Docker API and no persistence.
