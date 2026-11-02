# Milestone D: Git integration (implementation and evidence)

Status: **implemented and verified offline; attended only.** Everything below ran against real `git`, local
repositories, a loopback smart-HTTP server with Basic authentication, the fake Docker and Buildx, and a fake
Keychain. No live Docker, VM or provider (GitHub, GitLab) was involved. `Gate::shipping()` is unchanged: a live
Git deployment is refused by the shipping binary before a repository is contacted, and `--simulate` is the
offline path.

## 1. Boundary decisions recorded

* **Milestone C: implementation-complete by owner decision.** The deferred real Docker/BuildKit acceptance
  evidence (`docker history` and layers for a real secret mount, a real container's `docker inspect`, the
  Keychain locked mid-session) is preserved as outstanding, not claimed.
* **Gate U: NOT PASSED. Unattended operation is unproven.** No new experiment was run and no agent was built. Whether a
  non-interactive process can read the Keychain items (deploy credentials, secret versions) with the screen
  locked, after sleep, logged out or before login is unanswered. Therefore every automatic behaviour in this
  milestone is **attended only**: it happens while a developer-started Harpoon process is running ("automatic
  while Harpoon is running"), and nothing in the code, the CLI help or this document claims otherwise.
* **Milestone D is the active implementation milestone.** Milestone E and any background agent remain deferred.

## 2. Ponytail review (done first, built in)

| | |
|---|---|
| **KEEP** | A system `git` runner with a bounded private workspace; URL, ref and commit validation; credential handling; HMAC-SHA-256 and token verification; one normalized push event; a source record per deployment; foreground polling; a loopback listener. |
| **REUSE** | B14's event queue (delivery-id idempotency, held, selected, superseded, `resume`); the SQLite record pattern and replay checks; `Trigger::{Poll, Webhook, Manual}`; the Keychain `SecretStore`, secret ids and versions from Milestone C; the deploy-docker JSON parser; the existing `deploy` and `replace` lifecycle (a Git run checks out, then calls them). |
| **CUT** | A Git client trait (the tests use real `git`); an HTTP framework; a scheduler or daemon; a second queue; storing events outside B14's. |
| **DEFER** | Unattended execution (Gate U); GitHub/GitLab Apps, PR comments and commit statuses; a relay or tunnel for real provider deliveries; Milestone E UI; submodules and LFS. |
| **RISK** (handled) | Option injection through a URL or ref (a value starting with `-`, `ext::`, whitespace) is refused before Git runs; credentials in argv, URLs, logs or records; trust-on-first-use for SSH; an oversized repository or a hung `git`; a workspace or key file surviving a failure; a tampered or replayed webhook; the checkout workspace being swept while in use (found by the tests and fixed, see section 6). |

## 3. What a developer can do

```
harpoon-deploy repo-add    --name shop --url https://github.com/acme/shop.git --app APP --container-port 8080 \
                           [--username U] [--branch main] [--filter 'release/*']... [--webhook github|gitlab] [--known-host 'host ssh-ed25519 AAAA...']
printf %s "$TOKEN" | harpoon-deploy repo-secret --name shop --kind credential      # or --kind webhook; Keychain, stdin only
harpoon-deploy git-deploy  --repo shop [--branch B | --tag T | --commit FULL_SHA]    # manual, from a chosen revision
harpoon-deploy git-trigger --repo shop --delivery ID --revision FULL_SHA [--ref refs/heads/B]   # a CI job that can reach this machine
harpoon-deploy git-run     [--repo shop]                                              # execute what is queued
harpoon-deploy watch       [--repo shop] [--interval-secs N>=60] [--webhook-port P]   # foreground; polls, listens on loopback, runs the queue
harpoon-deploy repo-list
```

* **Git sources.** Public HTTPS; private HTTPS with a Keychain token; SSH with a Keychain private key and a **pinned host key
  (required; there is no trust on first use)**; `file://` for a repository on this machine; plain `http` only to a loopback host.
  A branch, a tag (annotated tags resolve to their commit) or an exact full commit id.
* **Checkout.** Host side, in `scratch/git-ws-<pid>-...` (mode 0700): a shallow fetch of the one revision into a work tree
  with no `.git`, size-watched while it arrives and bounded by a timeout that kills the whole process group. The result is
  handed to the existing `deploy` or `replace` (so the Dockerfile, target, build arguments, secrets and the Buildx request are
  Milestone B/C's, unchanged).
* **Immutable record.** A `gitsrc` record (repository, URL without credentials, branch/tag/commit asked for, the **resolved
  commit**, trigger, delivery id) is written **before** the deployment starts, keyed by the deployment id. Release identity is
  still the deployment id; `repo-list` shows the last deployment and commit. A branch that moves later does not change what a
  recorded deployment was.
* **Events and the queue.** One normalized `repository push` (provider, repository, ref, revision, delivery id) is queued with
  B14's `Event` records, idempotent on `(application, delivery id)`: a repeat is a duplicate and writes nothing; the same id
  with another revision or source is refused. A paused application keeps events `held` until `resume` selects one. An
  unpaused application executes the **newest** queued event (latest received) and records the older ones as `superseded`
  by it. A new `exec` record marks an event `executed` (with the deployment) or `failed` (with the reason, no deployment).
* **Attended polling.** `watch` runs `git ls-remote` per repository at least every 60 s (jitter up to 10 %), filters
  branches (the production branch, or glob filters such as `release/*`), compares with the last observed commit (`gitseen`) and
  queues `poll:<repo>:<commit>`. The first look at a branch records a baseline and deploys nothing. A change found by the
  first poll of a new process is **reported as `LATE`** (source `poll-late`): it landed while no Harpoon was running.
* **Webhooks.** `--webhook-port` binds `127.0.0.1` only (the API takes no address), reads at most 16 KiB of headers and
  256 KiB of body (refused before reading), with deadlines; GitHub's `X-Hub-Signature-256` is HMAC-SHA-256 over the raw body,
  GitLab's `X-Gitlab-Token` is compared in constant time; nothing is parsed before verification. A verified push is buffered
  in memory and queued when the running build finishes. Deletions, pings, tags and unrelated events are acknowledged and
  ignored.
* **CI-initiated requests.** `git-trigger` is the bounded, explicit mechanism: a CI job that can reach the developer's machine
  (for example over SSH) queues one event with its own delivery id; it needs the full commit id, is branch-filtered, and is
  idempotent. It deploys nothing by itself; `git-run` or a running `watch` does.

## 4. Security design

* **Credentials never in argv, URLs, logs or records.** A URL with a password is refused. An HTTPS token and username are
  files (0600, in a 0700 directory in the workspace) read by an askpass script (`GIT_ASKPASS`); the environment holds only
  file paths. An SSH key and the pinned `known_hosts` line are files; `ssh` runs with `StrictHostKeyChecking=yes`,
  the pinned file as the only host-key source, `IdentitiesOnly`, no agent, `BatchMode`. Git's configuration, hooks, credential
  helpers, terminal prompts and protocols other than file/http(s)/ssh are disabled. Git's output is redacted with the
  credential before it can appear in an error. The Keychain holds the credential and the webhook secret (versions as in C).
* **Cleanup.** The workspace is removed on success, failure, timeout and unwinding. A hard kill leaves it behind: workspaces
  are named by owner pid, so the next writing command or `recover` removes those whose owner is gone, `status` reports them,
  and a live process's workspace is never touched. Cleanup after a hard kill is **not guaranteed**; until the next writing
  command a credential file may remain on disk in a 0700 directory.
* **Bounds.** Default timeout 120 s and 256 MB (`--git-timeout-secs`, `--git-max-mb`); the fetch is killed when the object
  store exceeds the limit and the checked-out tree is measured as well.
* **Verification before the queue.** Only a verified push reaches `queue_event`; tampered, wrongly signed, unsigned,
  oversized and unrouted requests get 401/413/404 and write nothing.
* **Gate.** `git-deploy`, `git-run` and `watch` without `--simulate` are refused by the shipping binary before any
  repository is read; the acceptance binary's live gates apply to what they run.

## 5. Tests (all offline)

| Crate | Passing | New for D |
|---|---|---|
| deploy | 242 | URL/ref/commit rules, filters (glob), registration rules, Git record codec and replay; queue idempotency, newest wins, paused hold, failed execution, exec codec and replay, cut after every write |
| deploy-journal | 48 | Git records and queued/executed events replay through SQLite, fail-closed observation |
| deploy-git | 21 | SHA-256/HMAC vectors; webhook verification, limits, routing and the loopback listener; real `git`: branch/tag/commit checkouts, polling, refused input, no residue, oversized repository, credential files and their absence from argv and environment, SSH command and pinned host key, a hung `git` killed with its children, smart-HTTP Basic auth with revocation and rotation, an unreachable remote |
| deploy-cli | 182 (183 with `--features acceptance`) | the commit-to-Buildx-to-record workflow (live path against the fake Docker), late polling, CI triggers with filters and supersession, failed checkouts, a paused application, signed GitHub and token GitLab fixtures with tampering/replay/oversize, a private repository with a revoked and rotated token, workspace residue, two pushes during a running build (the build finishes, only the newest builds next), the interval floor and the documentation wording |

fmt, Clippy `-D warnings` and the CI source guards are clean for all affected crates.

## 6. Defects found and fixed while building

* The writer-open sweep removed a live checkout workspace (it assumed, as for build secrets, that holding the writer lock
  proves nothing is in use; a checkout outlives the lock). Workspaces are now owner-pid scoped.
* Polling created the scratch directory before the private-mode check.
* A header block larger than the cap was accepted when its terminator arrived in the same read.
* Reusing a workspace for a second Git operation tried to recreate its credential files.

## 7. Not done, not proven

* **Not exercised:** an SSH server (wrong host key, revoked deploy key over SSH): the command and the pinned `known_hosts` are
  asserted, not run; TLS certificate handling (https is Git's); real GitHub or GitLab deliveries (they need a tunnel or relay,
  deferred); GitLab's delivery-id header is **not confirmed** (it is used if present, otherwise derived from the commit);
  submodules and Git LFS (a checkout does not initialise them).
* **Queue durability:** webhook events buffered in memory while a build runs are lost if the process dies; polling finds the
  push again on the next run (reported late). Accepted for an attended process.
* **No cancellation:** an active build is never interrupted by a newer push (by design: the newest queued request builds next).
* **Large histories:** a fetch by exact commit may fall back to every branch head, bounded by the same limits.
* **Gate U** is still open: nothing here may be described as unattended.
