# Stage 5 — Bash / Tooling Audit

Status: audit complete; no production behavior, guest image, Docker Engine,
containerd, runc, Swift host runtime, or UI code was changed.

Date: 2026-10-13 (America/Chicago)

## Scope and evidence

The canonical checkout is `/Users/juancastro/Documents/Github/Harpoon`.
The project mirror attached to this chat contains only `AGENTS.md`, so the
actual checkout was located and its existing Stage 0–4 working-tree changes
were preserved. Stage 0–4 artifacts and source are the evidence of record.

The static inventory found 63 `*.sh`/`*.py` files under `tools/`, `harpoon/`,
and `ui/`, totaling 9,392 lines, plus three executable files without a
script extension under `tools/guest-builder/src/`, totaling 1,279 lines:

| Surface | Count | Lines | Primary conclusion |
|---|---:|---:|---|
| Build and guest-builder shell | 10 | 1,032 | KEEP_AS_SHELL, except guest `init` runtime sections |
| Release/package shell | 10 | 1,102 | BUILD_RELEASE_ONLY / KEEP_AS_SHELL |
| Validation and Stage 0–5 gates | 9 | 891 | TEST_HARNESS_ONLY / barrier logic |
| Regression and milestone harnesses | 34 | 6,367 | TEST_HARNESS_ONLY; historical disposition below |
| Python harness | 1 | 53 | TEST_HARNESS_ONLY |
| Guest runtime `init` | 1 | 944 | mixed; compiled-code candidate sections |
| Guest runtime `harpoon-mgmt` | 1 | 302 | transitional runtime candidate |
| Guest runtime wrapper | 1 | 33 | KEEP_AS_SHELL until guest service exists |

The first four rows are the 63 extension-based files. The last three rows
are not included in that count. `socat` itself is supplied by the guest root
and invoked from `tools/guest-builder/src/init`; it is not a repository source
file.

## Responsibility inventory and classification

### KEEP_AS_SHELL

These are linear, inspectable, bounded orchestration tasks. Rewriting them in
Rust or Swift would add a compiler/runtime dependency, platform-specific
distribution work, and a second implementation of existing native tools
without removing meaningful state:

- `harpoon/build.sh:1-21`: invokes `swiftc` with the repository's source list
  and links Virtualization.framework.
- `tools/guest-builder/build.sh:1-77`, `build-root.sh:1-157`,
  `build-initramfs.sh:1-336`, `fetch-kernel.sh:1-55`: compose a guest artifact
  from pinned inputs and existing host tools. The builder is orchestration;
  its manifest/hash generation is a build guarantee, not a runtime owner.
- `tools/guest-builder/sanitize-root.sh:1-119`: image sanitation and package
  setup. It is build-time mutation with a bounded command sequence.
- `harpoon/install.sh:1-43` and `uninstall.sh:1-45`: explicit user-facing
  installation/removal, including the necessary `sudo` boundary.
- `tools/release-preflight.sh:1-70`, `tools/release/macos/build.sh:1-52`,
  `build-dmg.sh:1-77`, `sign-app.sh:1-57`, `notarize.sh:1-59`,
  `release.sh:1-186`, `verify-app.sh:1-14`, and
  `verify-signatures.sh:1-78`: release sequencing and platform tools.
- `ui/harpoon-desktop/src-tauri/prepare-bundle.sh:1-41` and
  `ui/harpoon-desktop/scripts/sign-app.sh:1-62`: Tauri packaging/signing
  glue. The Rust backend already owns runtime argument passing and safe Docker
  command construction; these scripts should not absorb that logic.
- `tools/verify-bundle.sh:1-181` and
  `tools/release/verify-runtime-closure.sh:1-292`: artifact assertions over
  files, signatures, Mach-O metadata, manifests, and closure content. They
  should remain shell because their subjects are shell-native inspection
  tools and release artifacts.

### TEST_HARNESS_ONLY

The Stage gates, parity tests, live regressions, fuzz runner, and milestone
tests are consumers of the runtime, not runtime owners:

- `tools/stage-0-1-gate.sh`, `stage-2-resource-gate.sh`,
  `stage-3-guest-boot-gate.sh`, `stage-4-host-runtime-gate.sh`, and this
  Stage 5 gate: barrier orchestration and evidence checks.
- `tools/validate.sh:1-78`: canonical validation sequence. It correctly
  selects Node 20, then delegates to build, parity, frontend, and Cargo
  checks. It is a test/release entrypoint, not application logic.
- `harpoon/cli-parity-test.sh`, `regression-asset-resolution.sh`,
  `regression-bridges.sh`, `regression-foreground-status.sh`,
  `regression-host-path.sh`, `regression-memory-capacity.sh`,
  `regression-m17-offline.sh`, `regression-stage5.sh`, `fuzz-host-path.sh`,
  and `ui-test.sh`: regression and acceptance harnesses.
- `harpoon/m3-test.sh` through `m18-test.sh`, including
  `m15-balloon-test.sh`, `ec-test.sh`, and the M9 fixture server: milestone
  harnesses. Their repeated polling, `docker` calls, and permissive warnings
  are test policy, not production policy.
- `tools/guest-builder/regression-*.sh`, `test-harpoon-mgmt-wrapper.sh`,
  `test-harpoon-mgmt.py`, `verify-guest.sh`, and `verify-root.sh`: guest
  artifact and management regression/verification harnesses.

The harnesses are allowed to use shell pipelines and bounded polling because
their job is to exercise observable behavior. They should not be mistaken for
evidence that the same parsing or retry policy belongs in production.

### BUILD_RELEASE_ONLY

`harpoon/package.sh:1-54`, the `tools/release/` tree, guest-builder build and
verification scripts, and the Tauri bundle preparation scripts are release
responsibilities. Rewriting them in Rust/Swift would be worse: the current
workflow already calls `codesign`, `notarytool`, `otool`, `plutil`, `file`,
`tar`, `cp`/`ditto`, and image utilities directly. A compiled replacement
would still shell out to these platform tools, while adding argument marshaling
and a new binary to sign and distribute.

There are currently two release orchestrators: the npm/Tauri `build:release`
path (`ui/harpoon-desktop/scripts/release.mjs`) and
`tools/release/macos/release.sh`. The shell path calls runtime-closure and
bundle verification; the npm path does not enforce the same guest closure.
The shell path also repeats guest build/verification work before and after the
lower-level release helper. This is a `DELETE_OR_COLLAPSE_CANDIDATE`, not a
reason to rewrite either path in a compiled language: choose one authoritative
orchestrator later and make the other a thin wrapper or retire it.

### HISTORICAL_OR_MILESTONE_ONLY

The `m3`–`m18` scripts are historical milestone assets, not canonical runtime
entrypoints. Keep them when they cover a regression that is not represented
elsewhere; retire or collapse them when a newer named regression owns the same
guarantee. The strongest keep-regression assets are:

- `regression-bridges.sh`: explicit socket persistence, concurrent Docker
  clients, half-close/keep-alive behavior, and STOPPING cleanup.
- `regression-foreground-status.sh`: foreground status projections and stale
  socket behavior.
- `regression-host-path.sh` plus `fuzz-host-path.sh`: deterministic path
  translation and fuzz coverage.
- `regression-m17-offline.sh`: offline/recovery characterization.
- `regression-asset-resolution.sh` and guest-builder migration/runtime-repair
  regressions: packaged artifact and persistent-root migration guarantees.

`m3`–`m8` and much of `m9`–`m16` overlap newer named regressions and are
retire/collapse candidates after a deliberate coverage mapping. They are not
dead weight merely because they are old: each should be removed only after a
replacement test demonstrates the same failure contract. `m17`/`m18` remain
useful soak/characterization assets but contain environment-dependent retries
and warnings, so they are not suitable as a single release gate.

### DELETE_OR_COLLAPSE_CANDIDATE

No script is deleted in Stage 5. Candidates for a later cleanup pass are:

- overlapping M-series checks duplicated by the named regression scripts;
- repeated one-off JSON extraction pipelines in M-series harnesses;
- repeated prior-gate invocation once a single Stage 5+ barrier owns the
  dependency chain;
- legacy or unreachable host-forwarding compatibility already identified in
  Stage 4 (`_legacy_startHostPortForward` and legacy single-forward fields).

These are cleanup candidates, not evidence for changing production behavior.

### SECURITY_SENSITIVE_REVIEW

The following are findings to repair or explicitly accept later; none were
changed here:

1. `tools/guest-builder/src/init:122-186` parses manifest lines with shell
   `read` and uses path values in `/...` expansions. The current manifests are
   builder-generated, but the parser has no explicit field-count, path-traversal,
   NUL, or duplicate-entry policy. A future typed reader should reject malformed
   records before any copy/link operation.
2. `tools/guest-builder/src/init:819-837` starts a temporary `socat` preflight
   and stops it with `pkill -f "VSOCK-LISTEN:2375"`. Pattern-based process
   termination is broader than a PID-owned cleanup and can kill an unrelated
   matching process in a shared guest.
3. `tools/guest-builder/src/init:852` and `:913` launch long-lived `socat`
   processes in the background without retaining a PID/owner record. Readiness
   is inferred from markers and later probes; supervision and cleanup are not
   centrally represented.
4. `tools/guest-builder/src/harpoon-mgmt-wrapper:4-6` trusts environment
   overrides for marker and target paths. The wrapper is an internal guest
   entrypoint, but a future service should constrain executable and marker
   locations and use descriptor ownership rather than ambient path selection.
5. `harpoon/install.sh:25-36` and `uninstall.sh:17-30` correctly quote normal
   variables, but perform privileged copies/removal and recursive deletion.
   This is acceptable for explicit install/uninstall commands; it is not a
   runtime pattern to reuse.
6. `tools/verify-bundle.sh:44-175` uses `bash -c` with interpolated paths.
   The current `APP` argument is quoted inside the generated command strings,
   but this remains unnecessary shell interpretation and should become direct
   commands or positional arguments in a later hardening pass.
7. `tools/release/macos/verify-signatures.sh:69-70` uses the same interpolated
   `bash -c` pattern around `codesign`; release paths are normally controlled,
   but the pattern is avoidable.
8. `tools/stage-2-resource-gate.sh:20,56-57` expands Docker IDs into unquoted
   command arguments. Docker IDs are daemon output and expected to be safe,
   but array/line-safe handling would remove the assumption.
9. Several milestone tests use `docker ... sh -c`, `xargs sh -c`, `eval-like`
   pipelines, and unbounded `head`/`grep` extraction. These are test-harness
   risks, not production runtime injection paths, but they can make a failed
   test report the wrong subject or execute unexpected fixture input.
10. `harpoon/m7-test.sh:82-109` writes synthetic JSON into user/application
    runtime locations. It is an intentional test mutation, but its cleanup
    and target selection deserve isolation before unattended use.
11. `tools/guest-builder/sanitize-root.sh:10-55` accepts an arbitrary image
    path and embeds it in a privileged Docker `sh -c` command. The script is
    documented for the template image but does not enforce that scope; an
    unusual path can also alter the generated command. This is the most
    consequential host-side shell boundary found.
12. `tools/guest-builder/src/harpoon-mgmt:19-43` runs root guest commands and
    buffers all child output without a guest-side timeout or output cap. The
    host has bounded reads, but a guest child may continue after the host gives
    up. This is a protocol/resource risk, not a shell interpolation bug.
13. `tools/guest-builder/src/harpoon-mgmt:46-155` contains a dead `handle_shell`
    implementation while `:207-286` contains the active inline implementation.
    This is an approximately 110-line collapse candidate when the management
    boundary is next changed.
14. `harpoon/regression-stage5.sh:31-45` temporarily overwrites tracked
    `tools/guest-builder/src/init` with a broken fixture and restores it later.
    An interrupted run can leave production-adjacent source modified; keep it
    out of unattended regression until it tests a temporary copy or has
    unconditional restoration.

Quoting is generally disciplined in the build and guest-builder scripts:
paths are commonly quoted and `set -e`/`set -u` are used. The findings above
are specific exceptions or policy gaps, not a claim that the whole shell
surface is unsafe.

## Where shell has become application/runtime logic

The boundary is visible in `tools/guest-builder/src/init`:

| Source section | Responsibility | Classification | Future owner |
|---|---|---|---|
| `1-105` | mounts, block discovery, first-root population | KEEP_AS_SHELL / boot glue | shell |
| `106-196` | immutable payload and module manifest validation, hash/mode checks, atomic repair | COMPILED_CODE_CANDIDATE / SECURITY_SENSITIVE_REVIEW | Rust `harpoon-guest` |
| `197-231` | root handoff and post-pivot validation | mixed; simple handoff stays shell | shell, with typed event emission later |
| `232-289`, `676-686` | ext4 geometry parsing, resize, postcondition checks | COMPILED_CODE_CANDIDATE | Rust `harpoon-guest` |
| `292-416` | kernel modules, forwarding, VirtioFS, balloon setup | mixed; simple module/setup glue stays shell; failure policy moves later | shell plus guest service policy |
| `417-575` | interface selection, DHCP/DNS/HTTPS checks, fallbacks | COMPILED_CODE_CANDIDATE | Rust `harpoon-guest` |
| `576-728` | package/runtime closure decisions and `/run` reset | mixed; `/run` reset stays shell; closure decision is a candidate | Rust service for policy, shell for native commands |
| `729-759` | Docker launch and bounded readiness polling | COMPILED_CODE_CANDIDATE as supervision policy; Docker remains Docker-owned | Rust `harpoon-guest` |
| `760-878` | vsock preflight, Docker `socat` bridge, bridge/network probes | COMPILED_CODE_CANDIDATE for listener ownership/readiness; `socat` may remain a first implementation | Rust service or a tightly supervised helper |
| `888-935` | Python management checks, `socat` listener, retry attempts | COMPILED_CODE_CANDIDATE | Rust service; preserve protocol contract |
| `944` | two-second marker loop | HISTORICAL_OR_MILESTONE_ONLY / DELETE_OR_COLLAPSE candidate | structured event stream or remove |

`tools/guest-builder/src/harpoon-mgmt:1-302` is already application logic,
not just glue: it parses line-oriented management commands, performs Docker
exec/inspect behavior, allocates a PTY, proxies descriptors, and handles
signals. `harpoon-mgmt-wrapper:1-33` is small shell glue around it and may
remain shell until the compiled service owns the listener and child lifecycle.

The current Python+socat implementation is still acceptable for the present
behavior and deployment constraints, but it is clearly transitional for the
long term. Evidence: the guest already requires Python, `py_compile`, a
Python handler, `socat`, two background listeners, a wrapper, retry logic, and
separate marker-based readiness. That is a distributed runtime contract, not
one simple helper. It is not a reason to replace it in Stage 5; it is a reason
to keep the eventual migration boundary narrow and test the protocol first.

The management code also has a concrete consolidation target: the dead
`handle_shell` helper is about 110 lines, while the live implementation is a
second copy. That is independent evidence for “transitional,” not evidence
that Rust would automatically reduce total source size.

## Exact harpoon-guest migration map and consolidation estimate

The future Rust guest service would replace responsibilities, not Docker
Engine/containerd/runc:

| Current responsibility | Current files/sections | Estimated code removed or collapsed | Why estimate is credible |
|---|---|---:|---|
| phase/state/retry policy | `src/init:1-944`, repeated failure loops and markers | 180–260 shell LOC | repeated `while true` failure loops, marker branches, and ad-hoc phase flags become typed states/events |
| manifest reconciliation | `src/init:106-196`, builder manifests remain inputs | 90–130 shell LOC | two near-parallel Python/module manifest readers and repair loops can share one typed reader/repair path |
| disk reconciliation | `src/init:232-289,676-686` | 45–70 shell LOC | arithmetic, raw reads, resize retry/postconditions become one bounded function |
| network/readiness policy | `src/init:417-575,729-878` | 130–190 shell LOC | DHCP/DNS/HTTPS, Docker polling, vsock preflight, and failure loops become explicit bounded transitions |
| Docker vsock listener ownership | `src/init:819-878` and one `socat` process | 20–40 shell LOC plus process ambiguity | typed child/PID/supervision removes `pkill -f` and unowned background launch |
| management protocol/listener | `src/harpoon-mgmt:1-302`, wrapper `1-33`, init `888-935` | 302–335 Python/shell LOC, but replaced by Rust code | parser, PTY proxy, signal handling, listener, and readiness become one service; this is relocation, not free deletion |
| diagnostic loop | `src/init:944` | 3–8 shell LOC | replace with structured event or delete if no consumer remains |

Total current shell/Python surface that could plausibly disappear from the
runtime path is approximately **770–1,030 lines**, but only **470–690 lines**
are true consolidation. The remaining 300–340 lines of Python management
behavior must be reimplemented if the current exec/PTY protocol remains. A
Rust guest service therefore simplifies ownership and failure semantics more
than it reduces total source size. If it merely ports Python line-for-line,
the migration is not justified.

A narrower estimate from the current source is 600–750 lines of `src/init`
runtime coordination plus 302 lines of Python management and 33 lines of
wrapper. The difference between that figure and the 770–1,030-line estimate is
intentional: the larger range includes duplicated verifier/test scaffolding
that can be collapsed only after coverage is mapped, while the smaller range
counts only guest-runtime coordination.

## Duplication and guarantee analysis

| Guarantee | Existing build/release check | Runtime/test duplicate | Disposition |
|---|---|---|---|
| required guest files/modules | `verify-guest.sh`, `verify-root.sh`, `verify-runtime-closure.sh` | `init:106-196,581-675` rechecks commands, hashes, modes, versions | retain runtime checks for drift; collapse implementation later |
| Python management import/compile | builder manifests and closure verification | `init:888-905` runs file/import/`py_compile` checks | transitional duplication; replace with service health/readiness |
| root image geometry | root builder/sanitize checks and bundle size checks | `init:232-289,676-686` reads and resizes each boot | runtime-only resize stays; duplicate probe can be consolidated |
| Docker/package availability | packaged guest artifact and release closure | `init:576-675` probes packages and may use APK | preserve fallback until artifact identity is trusted; measure before removal |
| socket modes | Stage 0/4 live evidence and regression checks observe `0600` | source/runtime checks are permissive in some paths | security-contract review; make one owner authoritative later |
| lifecycle/readiness | Stage 0–4 barriers and regression scripts | Swift lifecycle, serial markers, status reconstruction, guest markers | duplicated authority; Stage 4 already records this as future review |

The shell gates themselves should not become production validation. Their
repeated prior-gate calls are barrier dependency checks and can be collapsed
only after the full program is complete.

## Recommended future responsibility split

| Layer | Keep | Move out / do not add |
|---|---|---|
| Shell | linear build, packaging, signing, installation, CI wrappers, test harnesses, early mount/pivot glue | state machines, unowned supervisors, protocol parsers, security-sensitive manifest handling |
| Swift host runtime | VM lifecycle, host disk provisioning, serial observation, host Unix/vsock bridges, port forwarding, lifecycle authority | guest package management, guest Docker supervision policy, Python management implementation |
| Rust `harpoon-guest` (if Stage 6+ evidence still supports it) | typed boot phases, manifest reconciliation, disk postconditions, bounded retries, Docker/vsock/management supervision, structured events | Docker Engine/containerd/runc implementation; no host daemon by implication |
| Python | temporary management compatibility only, isolated tooling/tests | long-lived guest control plane after migration boundary is proven |
| `socat` | acceptable short-term transport adapter while behavior is stable | final owner of process lifecycle, readiness, retries, or security policy |
| Test tooling | named regressions, milestone characterization, fuzzing, release verification, stage barriers | hidden production fallback behavior or duplicated runtime authority |

This preserves the Stage 4 conclusion: a persistent host daemon is not
justified. The compiled candidate is guest-side and should be introduced only
if a protocol-preserving migration reduces failure ambiguity and ownership
surface.

The active CI workflow (`.github/workflows/validation.yml:28-35`) currently
invokes `tools/validate.sh`, not the untracked Stage 0–5 barriers. The barriers
are still valid local audit gates, but they should not be described as CI
enforcement until intentionally wired into the repository workflow.

## Exit criteria and conclusion

1. Canonical shell/Python/socat/tooling surface inventoried: PASS.
2. Every relevant responsibility classified, including mixed sections: PASS.
3. Orchestration separated from runtime logic: PASS.
4. Future guest migration mapped to exact files/sections: PASS.
5. Scripts that should remain shell identified: PASS.
6. Duplicated guarantees identified: PASS.
7. Security/quoting/protocol risks documented without fixes: PASS.
8. Historical scripts classified keep-regression vs retire-candidate: PASS.
9. Python `harpoon-mgmt` + `socat` concluded acceptable now but transitional: PASS.
10. No rewrite/refactor performed: PASS.
11–14. Prior Stage 0–1, 2, 3, and 4 barriers rerun by the Stage 5 gate: PASS
    when the host permits the existing live checks; the deterministic artifact
    path remains available when live VM access is unavailable.
15. Stage 5 barrier: `STAGE_5_GATE_PASS`.

Stage 5 can be formally closed. Stage 6, UI work, and all production
refactoring remain explicitly stopped.
