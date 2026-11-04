# Editing a repository registration

`repo-update` (and Settings, Source, Edit registration in the desktop) changes where and how **future** deployments are built. It records a new version of the registration and does nothing else: no build, deploy, restart, replacement or Docker call.

## Contract

- **Append only, no schema change.** An edit appends one `gitrepo` record for the same name (the latest wins, as `repo-add` already does). Nothing is rewritten.
- **Identity is fixed.** The name, the application id and the credential settings (kind, username, pinned host key, webhook) never change. A rename would orphan the Keychain items, which are keyed by name, so it is not offered.
- **History is fixed.** Every deployment keeps the `gitsrc` record written before it started (repository, URL, commit). Releases keep their pin and configuration snapshot. The desktop reads a deployment's source from that record, not from the live registration.
- **Editable:** source URL, branch, Dockerfile path, container port, health check path. A value can be changed but not cleared. Context, target, filters, project, environment and webhook are untouched.
- **Versioned.** The registration's `at_ms` is its version. `--expect-registered-ms` is required; the edit is refused if the registration was edited or re-registered after the caller read it. The check is repeated under the journal writer lock, so two editors cannot overwrite each other. The new record's time is strictly greater than the old one.
- **Validated like registration.** A changed local (`file://`) source or branch is checked with the PR #160 check before the writer opens; a refusal writes nothing. Remote sources are not contacted ("not verified yet").
- **Confirmation.** Once the application has deployments the edit needs `--confirm-change` (the dialog's checkbox). The engine enforces it.
- **Credentials.** The token or SSH key stays attached to the repository name and belongs to its host. A credentialed registration cannot move to another host (the secret would be sent there); use a new repository. The edit neither reads nor deletes Keychain items. The Keychain service is global (`io.harpoon.deploy`), not per runtime directory, so an isolated test namespace shares secret names with production; this change does not touch that.
- **Held events.** A source change is refused while held or selected Git events exist, because they name revisions of the old source. Resolve them first. A branch-only change is allowed.

## Concurrency

- The journal writer lock is exclusive. A running deployment holds it, so an edit during a deployment is refused ("another Harpoon process holds the journal").
- A deployment reads the whole registration once. Because a registration is one record, it can never see a half-applied edit; the `gitsrc` it writes names the URL it actually used.
- Polling: an observation made before an edit is not a baseline for the new registration (`seen.at_ms >= registration.at_ms`), so a new source is not compared with the old one's commits; the next pass starts a fresh baseline and queues nothing. A poll that read the registration before an edit discards its result if the registration changed when it came to record it. `watch` re-reads the registration every pass.
