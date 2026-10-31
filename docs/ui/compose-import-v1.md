# Desktop Compose Import — v1

Status: implementation proposed in an isolated PR. No live Docker execution has been performed.

## Purpose and boundary

Containers offers **Create from Compose** in both empty and populated states. This is ordinary Docker Compose, not the Harpoon 2.1 deployment engine, and never opens its shipping gate.

1. Browse: native macOS folder picker; recognize conventional root-level Compose filenames. Select one base file and optional root-level override files explicitly; never load overrides silently.
2. Inspect: canonicalize folder/files, reject Compose files escaping the root, run `docker --context harpoon compose ... config --format json` using the discovered Docker CLI. Return only service names, image/build, ports, mounts, profiles, networks and volume names. Do not return environment values.
3. Review: show project name, context, selected files, resources, warnings, and potential build/pull/create side effects. Existing Compose projects are blocked by project label, rather than recreated. Unsupported UDP or explicit host-IP mappings are blocked.
4. Launch: explicit Create and Start button; revalidate source, Compose config fingerprint, Harpoon context endpoint and existing-project check. Run `compose up -d --build --no-recreate --quiet-pull --quiet-build` through the Harpoon context. Never run down, prune, force, remove volumes, or auto-retry.
5. Inspect: refresh Docker inventory and select the new stack in the existing Containers view. Docker labels and inspect data remain authoritative.

## Caveats and deferred work

- The selected Compose YAML is trusted local developer configuration, not sandboxed code. Compose may reference local files and mounts outside the selected folder. Display mounts for review.
- Profile-gated services are visible but are not started by default.
- A build or start error may leave partial Docker resources. Report the failure honestly; do not clean up without another explicit action.
- Imported folder and selected files are not yet persisted as a reopenable saved source reference. The running stack remains visible via Docker labels.
- Live build logs, profile editor, nested compose source files, arbitrary custom overrides, Compose down, DNS, and automatic recovery are not in this slice.
- The macOS picker uses the system folder chooser through osascript and requires Finder-launched validation.
- Compose remains authoritative; this interface does not implement its own YAML parser or Docker container runtime.

## Validation needed before merge

- Frontend: npm run build && npm test; Rust: cargo fmt --check && cargo test.
- Finder-launched picker: select/cancel; invalid/competing files; symlink escape; keyboard navigation and narrow layouts.
- Read-only inspect: verify no Docker side effects; detect missing env and invalid config; confirm secrets absent from review output.
- Fresh disposable fixture only, with explicit separate authorization: baseline inventory, review, one launch, resulting stack, no unrelated resource mutation.
- Negative: same-name project blocked, changed config blocked, unsupported port bindings blocked, partial failure reported, no hidden cleanup.

No action in this PR authorizes changes to a currently running Harpoon VM.