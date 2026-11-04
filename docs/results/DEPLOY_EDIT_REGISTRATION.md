# Editing a repository registration

`repo-update` (and Settings, Source, Edit registration in the desktop) changes where and how **future** deployments are built. It records a new version of the registration and does nothing else: no build, deploy, restart, replacement or Docker call.

## Contract

- **Append only, no schema change.** An edit appends one `gitrepo` record for the same name (the latest wins, as `repo-add` already does). Nothing is rewritten.
- **Identity is fixed.** The name, the application id and the credential settings (kind, username, pinned host key, webhook) never change. A rename would orphan the Keychain items, which are keyed by name, so it is not offered.
- **History is fixed.** Every deployment keeps the `gitsrc` record written before it started (repository, URL, commit). Releases keep their pin and configuration snapshot. The desktop reads a deployment's source from that record, not from the live registration.
- **Editable:** source URL, branch, Dockerfile path, container port, health check path. A value can be changed but not cleared. Context, target, filters, project, environment and webhook are untouched.
- **Versioned.** The registration's `at_ms` is its version. `--expect-registered-ms` is required; the edit is refused if the registration was edited or re-registered after the caller read it. The check is repeated under the journal writer lock, so two editors cannot overwrite each other. The new record's time is strictly greater than the old one.
- **Validated like registration.** The Dockerfile path and health check path are checked with the rules `deploy` applies (a plain relative Dockerfile path inside the build context; a health path that starts with `/` and has no control character or space), so an edit cannot store a value deploy would refuse. These checks are in `repo-update` only, not in journal replay, so existing journals stay readable. A changed local (`file://`) source or branch is checked with the PR #160 check before the writer opens; a refusal writes nothing. Remote sources are not contacted ("not verified yet").
- **Confirmation.** Once the application has deployments the edit needs `--confirm-change` (the dialog's checkbox). The engine enforces it.
- **Credentials.** A saved token or SSH key was issued for one *destination*: transport, host and port (`harpoon_deploy::git::destination`). An edit may change a credentialed registration's address only to one with the **same destination**; otherwise it is refused and the person adds a new repository. The edit neither reads nor deletes Keychain items, and no error or test output carries a secret.
  - The address must be readable unambiguously as `[user@]host[:port]` over https, http or ssh, or scp-style `[user@]host:path`. The parser is deliberately stricter than the URL grammar, because Git hands the URL to libcurl and OpenSSH: for `https://evil.example#@good.example/x` (or `?@`) libcurl contacts `evil.example`, while a parser that reads "after the last `@`" names `good.example`. So an authority containing `# ? \ %`, whitespace, a second `@`, a password, an empty or non-numeric port, an empty, trailing-dot or non-ASCII host label, or an unterminated IPv6 literal names **no** destination, and no destination is never equal to another (`None == None` is a refusal).
  - Compared after normalisation: host (lower-case), port (the scheme default when absent: 443, 80, 22), and transport. So `https://h/x`, `https://H:443/y` and `https://u@h/z` are one destination; a different port, `https` to `http` (including loopback), or `https` to `ssh` is not.
  - An unchanged address is never blocked by this rule, so a registration made before it existed (even with an unreadable address) can still have its port or branch edited, but its address cannot move.
  - **Not covered:** `repo-add` with an existing name still replaces a registration (the pre-existing, documented meaning for scripts) and is not subject to this rule; see the PR for the proposed follow-up. The desktop always creates with `--new` and never replaces.
  - The Keychain service is global (`io.harpoon.deploy`), not per runtime directory, so an isolated test namespace shares secret names with production; unchanged here.
- **Held events.** A source change is refused while held or selected Git events exist, because they name revisions of the old source. Resolve them first. A branch-only change is allowed.

## Concurrency

- The journal writer lock is exclusive. A running deployment holds it, so an edit during a deployment is refused ("another Harpoon process holds the journal").
- A deployment reads the whole registration once. Because a registration is one record, it can never see a half-applied edit; the `gitsrc` it writes names the URL it actually used.
- Polling: an observation made before an edit is not a baseline for the new registration (`seen.at_ms >= registration.at_ms`), so a new source is not compared with the old one's commits; the next pass starts a fresh baseline and queues nothing. A poll that read the registration before an edit discards its result if the registration changed when it came to record it. `watch` re-reads the registration every pass.
