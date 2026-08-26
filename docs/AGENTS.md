# Harpoon Agent Instructions

This file defines the standing engineering rules for any AI coding agent working in the Harpoon repository.

These instructions apply to every task unless the user explicitly overrides them.

---

# 1. Read Before Acting

Before modifying code, documentation, tests, scripts, generated artifacts, or repository configuration:

1. Read this `AGENTS.md`.
2. Read `docs/roadmap.md`.
3. Inspect the current repository state with Git.
4. Read documentation relevant to the requested milestone.
5. Inspect the implementation being changed rather than relying on previous summaries.
6. Review existing test/result evidence for the affected subsystem.

Do not assume a previous agent's report accurately represents the repository.

Repository contents and executable evidence are authoritative.

---

# 2. Ponytail Is Mandatory

Use the **Ponytail skill on every engineering run**.

Ponytail must be applied before committing to an implementation approach.

Use it to challenge:

- architectural assumptions,
- unnecessary complexity,
- hidden coupling,
- premature abstraction,
- incorrect ownership boundaries,
- unsupported platform assumptions,
- security implications,
- failure modes,
- misleading benchmarks,
- scope creep,
- whether a proposed change solves the actual problem.

Do not use Ponytail merely as a final review.

It should influence planning before implementation begins.

If Ponytail materially challenges the proposed approach, investigate the challenge before proceeding.

If the Ponytail skill is unavailable in the current environment, explicitly report that fact rather than silently skipping it.

---

# 3. The Roadmap Is Canonical

`docs/roadmap.md` is the canonical project roadmap.

Every agent must read it before beginning work.

Do not infer the current milestone from conversation history, old reports, filenames, or memory when the roadmap can answer it.

When work materially changes project state, update `docs/roadmap.md` as part of the same task.

Examples include:

- milestone started,
- milestone completed,
- milestone changed from PASS to CONDITIONAL or vice versa,
- blocker discovered,
- blocker resolved,
- architectural assumption invalidated,
- new evidence changes milestone ordering,
- a milestone is split, merged, removed, or added,
- the next actionable milestone changes.

Do not update roadmap status based on intention.

Status must reflect repository evidence.

Do not mark a milestone PASS merely because code compiles or a harness exists.

---

# 4. Evidence Over Narrative

Harpoon development is evidence-driven.

Prefer:

- executable tests,
- captured logs,
- hashes,
- measured timings,
- measured resource behavior,
- explicit state transitions,
- reproducible commands,
- Docker API responses,
- host-visible measurements,

over assertions in prose.

Previous summaries are context, not proof.

When documentation conflicts with executable evidence, investigate the conflict and correct the documentation.

Preserve useful historical evidence, but clearly distinguish historical failures from current behavior.

Never fabricate missing measurements.

Use classifications such as:

- PASS
- CONDITIONAL PASS
- BLOCKED
- INCOMPLETE
- FAIL

according to actual evidence.

---

# 5. Preserve Baselines

Do not overwrite useful benchmark or acceptance evidence merely to produce cleaner results.

Before replacing existing result data:

- determine whether it is authoritative,
- archive it when appropriate,
- keep before/after measurements distinguishable,
- preserve provenance.

Performance changes should have reproducible before/after evidence.

A benchmark harness must not silently convert:

- missing process data,
- missing logs,
- dead runtimes,
- failed workloads,
- unavailable VM metrics,

into numeric zeroes.

Missing evidence is missing evidence.

---

# 6. One Architectural Seam at a Time

Harpoon is developed incrementally.

Do not expand a task into unrelated architecture work.

When working on a milestone:

- solve the milestone,
- verify it,
- document the result,
- update the roadmap,
- stop at the stated boundary unless explicitly instructed otherwise.

Avoid speculative frameworks for future requirements.

Do not build abstractions for hypothetical backends, platforms, VM types, orchestration systems, or runtimes without demonstrated need.

---

# 7. Product Boundary

Harpoon is:

> A lightweight Docker-compatible development runtime for Apple Silicon macOS built around Apple's virtualization facilities, with host resource efficiency—especially post-workload memory behavior—as a primary product goal.

Docker remains authoritative for Docker state.

Linux remains authoritative for container isolation.

Apple virtualization APIs remain authoritative for virtualization.

Harpoon owns the macOS/Linux boundary and Harpoon runtime lifecycle.

Do not create a competing container abstraction.

Do not unnecessarily reimplement mature Docker, OCI, containerd, BuildKit, or Linux functionality.

---

# 8. MVP Discipline

The core MVP is:

> Ordinary local Docker development on Apple Silicon with substantially better resource behavior and without restrictive runtime licensing.

Prioritize:

- Docker API compatibility,
- Docker CLI compatibility,
- Docker Compose compatibility,
- persistent Docker state,
- localhost published ports,
- bind mounts suitable for normal development,
- reliable lifecycle behavior,
- low idle overhead,
- host-visible memory reclamation,
- developer ergonomics.

Do not allow unrelated features to delay those goals.

Unless the roadmap explicitly changes scope, avoid introducing:

- Kubernetes,
- multi-VM orchestration,
- Intel Mac support,
- Windows containers,
- arbitrary guest distributions,
- custom container engines,
- custom image formats,
- cloud orchestration,
- plugin ecosystems,
- production GUI work ahead of runtime maturity.

---

# 9. Measure Before Optimizing

Do not optimize based solely on intuition.

For resource or performance changes:

1. identify the suspected cost,
2. establish a trustworthy baseline,
3. make the narrowest reasonable change,
4. measure again,
5. compare,
6. check regressions,
7. retain or revert based on evidence.

Distinguish carefully between:

- configured VM memory,
- guest memory,
- guest available memory,
- guest cache,
- ballooned memory,
- container memory,
- host VM resident memory,
- Harpoon process RSS,
- macOS compressed memory,
- host swap,
- host memory pressure.

Guest-visible free memory is not proof that macOS reclaimed memory.

---

# 10. Do Not Destroy Useful Linux Cache Without Evidence

Linux page cache is useful.

Do not introduce explicit cache dropping into normal Harpoon operation merely to make memory measurements appear better.

Any cache-dropping policy requires evidence demonstrating that:

- supported reclamation mechanisms are insufficient,
- host-visible memory behavior materially improves,
- ordinary development performance is not unacceptably harmed.

Treat such behavior as experimental until proven otherwise.

---

# 11. Security Review Is Part of Every Change

Before completing a task, review the resulting diff as though it came from an untrusted contributor.

Check for:

- unexpected network access,
- telemetry,
- credentials,
- tokens,
- remote endpoints,
- unnecessary dependencies,
- install hooks,
- shell injection,
- unsafe temporary files,
- unsafe socket permissions,
- Docker socket exposure,
- privilege escalation,
- broad macOS entitlements,
- launch agents,
- persistence mechanisms,
- opaque binaries,
- unverified downloaded artifacts,
- unexpected executable files.

Use the minimum macOS entitlements and permissions necessary.

Do not silently add telemetry.

Do not expose the Docker API beyond the boundary required by Harpoon.

---

# 12. External Artifact Provenance

Do not silently download or commit opaque guest artifacts.

For kernels, initramfs images, root filesystems, guest packages, binaries, or similar artifacts, document where practical:

- source,
- version,
- architecture,
- checksum,
- licensing/provenance,
- acquisition/build procedure.

Generated or large runtime artifacts should normally remain outside Git and be ignored appropriately.

Never replace provenance with "download latest."

---

# 13. Apple Virtualization Assumptions

Do not revive disproven architectural claims.

Current repository evidence, rather than historical speculation, determines platform capability.

When Virtualization.framework produces transient failures:

- preserve diagnostics,
- classify them accurately,
- avoid claiming a root cause without evidence,
- do not redesign the virtualization architecture solely because of an unexplained transient.

Do not substitute another hypervisor or virtualization backend without an independently demonstrated requirement.

---

# 14. Harnesses Must Be Trustworthy

Test code is production engineering evidence.

A test harness must:

- test the intended binary,
- record binary provenance when variants matter,
- use authoritative runtime paths,
- fail fast when the runtime disappears,
- distinguish infrastructure failure from product failure,
- retain useful logs,
- use bounded timeouts,
- return meaningful exit status,
- avoid stale data contamination,
- avoid measuring unrelated processes,
- avoid silently falling back to another Docker runtime.

When Docker Desktop or another Docker daemon exists on the host, explicitly prove Harpoon tests are using Harpoon's endpoint.

---

# 15. Documentation Must Follow Evidence

Update relevant documentation when implementation evidence changes architectural understanding.

Likely documents include:

- `docs/roadmap.md`
- `docs/architecture.md`
- `docs/requirements.md`
- `docs/memory-model.md`
- `docs/performance.md`
- `docs/risks.md`
- `docs/decisions/`
- `docs/results/`

Do not mechanically update every file.

Update only documents whose claims materially changed.

Historical result documents may remain historical; annotate or supersede them rather than rewriting history when appropriate.

---

# 16. Keep Runtime and UI Boundaries Separate

The eventual GUI is a client of Harpoon.

The GUI must not become the owner of:

- VM lifecycle,
- Docker state,
- persistent runtime state,
- networking,
- memory policy.

Closing a GUI must not terminate the runtime.

Do not pull GUI concerns into runtime milestones prematurely.

---

# 17. No Hidden Cleanup

Do not delete unfamiliar files or artifacts simply because they look stale or suspicious.

Determine what they are first.

Do not perform destructive cleanup of:

- user Docker state,
- Harpoon disk images,
- volumes,
- persistent configuration,
- benchmark evidence,

without explicit justification.

Cleanup scripts must distinguish Harpoon-owned ephemeral state from persistent user state.

---

# 18. Git Discipline

Before and after meaningful work, inspect:

    git status
    git diff

Understand unrelated existing changes before touching them.

Do not silently revert another contributor's work.

Keep generated artifacts and local runtime files out of source control unless they are intentionally tracked evidence.

When a milestone is complete, make sure the repository diff tells the same story as the final report.

---

# 19. Completion Criteria

Before reporting a task complete:

- implementation exists,
- relevant tests were actually run,
- results were inspected,
- failures are classified,
- regressions were checked,
- security review was performed,
- documentation was corrected where necessary,
- `docs/roadmap.md` reflects the new project state.

Compilation alone is not completion.

A test harness existing is not completion.

A VM entering `Running` is not proof of guest userspace.

A socket accepting connections is not proof of Docker compatibility.

Guest memory becoming free is not proof that macOS reclaimed memory.

Use the acceptance criteria belonging to the current milestone.

---

# 20. Final Agent Report

At the end of a meaningful engineering task, report enough information for the next agent to resume without reconstructing the session.

Include:

- what was investigated,
- Ponytail findings that materially affected the approach,
- files materially changed,
- runtime behavior changed,
- tests run,
- measured results,
- failures or external blockers,
- security/provenance findings,
- roadmap status change,
- exact next actionable milestone or experiment.

Do not disguise uncertainty.

Do not call blocked work PASS unless the roadmap's acceptance criteria explicitly permit that classification.