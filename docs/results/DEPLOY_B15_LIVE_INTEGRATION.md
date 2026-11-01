# B15: reviewed live lifecycle integration (acceptance build only)

Status: **implemented and verified offline.** The live `replace`, `rollback` and `restore` paths that passed L1-L4 on
a local experimental binary are now in the maintained code, behind a gate that stays closed in the shipping binary.
Nothing here ran against Docker; no live acceptance was repeated; Milestone B is **not** accepted.

## 1. What moved from the experiments, and what did not

Not a cherry-pick: the experimental branch predates B13 and B14 and merges into current `main` with real conflicts
(`commands.rs`, `lib.rs`, `tests/cli.rs`). Only the live wiring was re-applied to current code.

| Moved (adapted) | Where |
|---|---|
| Live gate check: build gate, exact authorization phrase, explicit `--docker-socket`, `--forwarder-pid`, `--mgmt-socket`, `--resources-dir` | `live_replacement_gate` |
| Real Docker backend selection, built exactly as a live `deploy` builds it (all live requirements on, forwarder ownership proof, guest free-space probe) plus replacement enabled | `live_backend` |
| Backend for `rollback`/`restore` that reads no project directory: image, port, health check, limits **and environment** all come from the pinned release | `pinned_only_cfg`, `DockerBackend::with_rollback` |
| `Kind::Docker` writer for live runs, `Kind::Simulated` for `--simulate` | `runtime_kind` |
| Live arguments for `rollback` and `restore` | `args.rs` |
| The offline sequence tests for the live wiring against the fake Docker | `tests/b15` |

| Deliberately **not** moved | Why |
|---|---|
| `--experimental-fail-final-check` and `LiveProbe` (the L2 synthetic final-check failure) | test-only fault; stays a local overlay |
| `--experimental-crash-after-stop` and `crash.rs` (the L4 SIGKILL) | test-only fault; stays a local overlay |
| `Gate.experimental_faults`, `experiment_main.rs`, `tests/experimental/l4.rs` | only exist to support the above |
| The experiment's `status`/hint edits | already on `main` (PR #131), in newer form |

## 2. What was adapted for B13 and B14

- **B13 `--env` and v2 pins:** a live `replace` hands the validated `--env` to the real backend through `DockerConfig.env`
  (the simulated path used `FakeBackend::with_env`; without this the live candidate would have run with no variables). A
  restoration uses the restored release's own pinned environment; a rollback uses the target's. Tests cover an empty value, a
  value containing `=`, and a v1 (no environment) pin.
- **B14 pauses:** `record_pauses` runs after every live command exactly as after a simulated one; a failed final check, an
  interrupted-then-restored cutover and a successful rollback each leave a durable pause. No live command reads, resumes or
  executes a held or selected event (tested: the control records are identical before and after).
- **Preliminary-failure next actions and wording:** `print_prepare_failure` now receives the real mode (it was hard-coded
  `simulated`), so a live run is never told to use `--simulate`; "(simulated)", "SIMULATED, no Docker" and "simulated clock"
  appear only for `--simulate`. `status` and `releases` name the live command only in a build whose gate is open; the shipping
  build keeps "this build cannot ... a live runtime".
- **Blocker text:** the stale "blocked until the bounded cutover recovery exists" now points at `recover` and the explicit,
  one-attempt `restore`.
- **Locking and replay:** unchanged. The writer is opened after every read-only refusal; a held journal stops every live command
  before Docker is contacted.

## 3. Why shipping stays closed

- `Gate::shipping()` has `live_deploy_enabled: false` and `live_replacement_enabled: false`. `src/main.rs` uses it
  unconditionally: no `cfg`, no feature, no environment read. No flag or environment variable can open a gate (a spawned
  shipping binary was run with `HARPOON_ACCEPTANCE=1`, `HARPOON_LIVE=1`, `DOCKER_HOST=...` and still refused with exit 6).
- No library source branches on a Cargo feature, so `--all-features` cannot change the shipping binary.
- An acceptance decision is a separate, explicit change to `Gate::shipping()`; this PR does not make it.

## 4. The acceptance executable

`harpoon-deploy-acceptance` (`src/acceptance_main.rs`) opens `live_deploy_enabled` and `live_replacement_enabled` and nothing
else. It exists only with the opt-in Cargo feature `acceptance` (`required-features`), and contains **no fault injection,
crash control or synthetic health failure**. It still requires the exact authorization phrase and every explicit path; it never
switches Docker context or socket implicitly (the CLI reads no environment).

Build (clean checkout of the reviewed commit, offline):

```
cd deploy-cli
HARPOON_ACCEPTANCE_REV=$(git rev-parse HEAD) cargo build --offline --release --features acceptance --bin harpoon-deploy-acceptance
shasum -a 256 target/release/harpoon-deploy-acceptance
target/release/harpoon-deploy-acceptance --acceptance-info
```

Record the revision, the SHA-256 and the toolchain with the evidence. Running it against Docker needs its own written authorization.

## 5. Build and packaging isolation evidence

Clean target directory, this commit:

- `cargo build --release`: produced `harpoon-deploy` only; no `harpoon-deploy-acceptance`.
- `cargo build --release --all-features`: also produced `harpoon-deploy-acceptance`. This is expected for that crate; the
  boundary is that nothing packages it (below), and the `harpoon-deploy` built in that same invocation still refuses
  `restore --apply` with "live restoration is not enabled in this build".
- Nothing that packages a release builds or names it: the DMG workflow builds only `ui/harpoon-desktop/src-tauri`; the Tauri
  bundle resources, `tools/release/**`, `tools/*.sh` and `package.json` never mention `deploy-cli`, `harpoon-deploy` or the
  acceptance executable. Release packaging therefore allowlists by construction (it names no deploy binary).

Enforced by tests (`tests/b15/packaging.rs`) and CI (`.github/workflows/deploy-cli.yml`):

- the bin declares `required-features = ["acceptance"]`; there is no `default` feature; there are exactly two bins;
- `main.rs` has no `cfg`/feature/environment dependency; the shipping gate literal is closed; no source branches on a feature;
- no file under `.github/workflows`, `tools`, `ui/harpoon-desktop/src-tauri`, `dist` or `package.json` names the acceptance
  executable, except the deploy-cli workflow, which builds and tests it and does not upload it, and which never uses
  `--all-features` or a release build;
- CI builds by default and asserts the acceptance binary is absent, then builds/tests/lints with `--features acceptance` and asserts it exists;
- the acceptance source contains no fault or crash control, and no production deploy source contains the experimental flags,
  `INJECTED FAULT`, `CrashAfterStop` or `SIGKILL`. The CI guard is scoped to the four crates' `src/` directories, so the
  legitimate crash-recovery tests (which cut a journal or a fake write) are not scanned.

## 6. Offline tests (17 new, plus 1 built only with the feature)

Real processes where the boundary is the point (the shipping binary; the acceptance binary under the feature); the library
in-process against the fake Docker for the rest. No fault injection: failures are an application that does not answer.

Shipping refusal of all three (in process and as a spawned binary, with environment variables set); unauthorized/incomplete
requests change nothing (journal bytes and request count identical); unconfirmed replacement stops nothing and pins `--env`;
preliminary failure keeps the previous release, no pause, no `--simulate` advice; healthy replacement with a measured
interruption, unique per-container markers and the new pin; failed final check, one bounded restoration from the old pin
(not the run's variables), durable pause, held selection untouched; explicit restoration dry-run first, one attempt, v1 and v2
pins, refused repeat without contacting Docker, replay afterwards; foreign container on the pinned port left alone; rollback
from the target's own pin (success) and untouched serving release (failure), refused unprovable target; writer contention
before Docker; live-versus-simulated reporting; no experimental flags in the production parser; bystander never touched.

## 7. Remaining Milestone B acceptance gaps (unchanged by this PR)

Live L5 (`--env` through replace, restoration and rollback), L6 (preliminary failure with a running release), L7 (state-loss
visibility), P1 (build overhead <= 1.2x), P2 (no resident process); live evidence for `restored_unverified`, `restore_failed`
and `not_attempted`; the decision on how to obtain a failing final check for L5's restoration (the fault overlay, or a natural
failing application); the written acceptance review; and the decision to change `Gate::shipping()`.

## 8. Updated live plans (each needs its own authorization)

Build one acceptance binary from the merged commit and record its SHA-256 first. Fresh application plus a bystander; dry-run-first cleanup; no prune.

- **L5:** `deploy --env A= --env B=x=y`, then `replace --env C=3 --confirm-cutover`; `docker inspect` each release for names and values; a failed
  cutover (overlay fault or natural) for one restoration; `rollback` to the first release. Each container carries its own pin.
- **L6:** a failing health check while the previous release runs: previous container id and state unchanged, candidate removed, no pause, next actions shown.
- **L7:** move the runtime directory aside (never delete), `status --docker-socket` lists labelled workloads read-only, restore the directory, journal hash equal; a corrupt copy fails closed.
- **P1/P2:** as in `DEPLOY_B13_ACCEPTANCE.md`.
- Stop on: any bystander change, an unexpected container or image, a second restoration attempt, an authorization mismatch.

## 9. The experimental branches

`experiment/milestone-b-live` and `experiment/b7-live-gate` are superseded by this PR and can be deleted locally. Keep
`experiment/milestone-b-l4` until the live evidence that needs its fault overlay (an L5 restoration, other recovery outcomes) is
collected; it must stay unpushed and unmerged. After that, delete it; its results are already recorded in `docs/results`.
