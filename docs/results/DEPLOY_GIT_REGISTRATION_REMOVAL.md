# Removing a Git registration: reader first (PR A)

Removing a registration disconnects an application from its repository. It does not delete the application, its
deployments, its serving release, any Docker resource, or any saved credential. It is a different thing from
deleting an application, which this work does not touch.

This is delivered in two steps because the journal cannot be read by a build that does not know the record:

| Step | Contents | Writes a removal? |
|---|---|---|
| **PR A (this document)** | the `gitdrop` record, replay, snapshots, race protections, identity and credential rules, a journal-schema check | **No.** No command, no desktop action |
| PR B | `repo-remove`, the confirmation dialog, capability gating | yes, behind a gate |

## The record

`gitdrop` (version 1): `name`, `app`, `registration` (the `at_ms` of the registration removed, its version), `url` and
`cred` (the destination a saved credential was issued for), `at_ms`. It carries no secret. It is framed and
checksummed like every other record. Decoding refuses a wrong field count or version, an invalid name, application id
or URL, and an unknown credential kind.

## Replay (all-or-nothing, as for every record)

A journal that cannot be replayed is refused whole. The rules, in `GitState::apply`:

* a removal must name a **live** registration with the same application, the same version (`registration == at_ms`),
  and the same URL and credential kind; otherwise the journal is invalid. A second removal of the same registration
  is therefore stale;
* a removal deletes the registration and **its polling observations**, and appends to `retired` (never pruned);
* an observation or a deployment source for a repository that is not registered is invalid, which is why the
  writers below must check first;
* **only the original name may register a removed application again** (`retired_owner`), and a different name
  naming it is invalid;
* `Source` records written before a removal stay: a deployment keeps its recorded repository, URL and commit.

## What PR A changes in behavior

* **`run_source`** (`git-deploy`, `git-run`) re-checks, under the journal writer lock, that the registration is still
  exactly the one the deployment was prepared from, before it appends the deployment's source. Removed: "was removed
  while its source was being fetched"; edited: "was changed". Either way nothing is recorded or deployed. Before this,
  nothing could remove a registration, so the gap was unreachable; with removals it would have made the journal
  unreadable.
* **Polling** discards a result for a removed registration (it already did for an edited one). **`watch`** stops
  polling a name once it is removed. `git-trigger` and webhook handling already looked the registration up under the
  writer lock and ignore a removed name.
* **Commands aimed at a removed name** (`git-deploy`, `git-trigger`, `repo-secret`, `repo-update`) say it was removed
  and that the application keeps its history.
* **`repo-add`**:
  * naming a removed application with `--app` is refused for any other name;
  * the original name needs `--app`, **`--confirm-reclaim`**, and no held or selected Git events from before the
    removal; omitting `--app` creates a **fresh application** and inherits nothing;
  * a saved credential keeps its destination across removals: the removed registration's address is the anchor, so
    registering the name again at another host, port or protocol is refused, with or without `--username`, however
    many times the name is removed and registered. Presence of a credential is read from the journal (`SecretSaved`),
    never from the Keychain, and removal never deletes a secret.
* **`repo-list`** shows removed registrations as `(removed: source disconnected)` with the last source.
* **Desktop**: snapshots carry `apps[].disconnected` (name, last source, versions); an application whose registration
  was removed keeps its readable name, shows **Source disconnected**, and Deploy, Redeploy and Retry are disabled with
  the reason. Rollback is not. Applications that never had a registration get the same gating (before, they were
  refused late by the engine). There is no removal action.

## The compatibility boundary (one-way)

Every build that reads the deployment journal (CLI, desktop, acceptance binary) shares these crates; the Swift engine
does not read it. A build **older than PR A** meeting a `gitdrop` fails to decode it and refuses the whole journal
("invalid SQLite record"), for reads and for writers alike, **before** any write. So an older build fails closed and
cannot corrupt a journal; it simply cannot open it. That is the boundary: once a journal holds a removal, only PR A
and later builds can use it, and there is no downgrade for that journal.

PR A also teaches the reader to look at the journal schema (`schema_migrations`). It reads up to **schema 2**
(`SUPPORTED_SCHEMA`) and refuses anything newer by name ("written by a newer Harpoon… Update Harpoon. The journal is
not damaged and was not changed"). Schema 1 is every journal today. Nothing writes schema 2 yet.

Verified before any removal is emitted: this build replays copies of the production desktop journal and the isolated
test journal (read-only, on copies, originals unchanged by hash), the existing journal suites, and the desktop
snapshot contract (additive `disconnected: null`).

## What PR B must do

1. **Gate the writer.** Add `repo-remove` behind an explicit capability, closed in shipping builds (the model is
   Gate U / `live_triggered_enabled`): a build that has not been through a release containing PR A must not be able to
   emit a record its siblings cannot read. Opening the gate is an operator decision, taken only when every build that
   reads the journal is PR A or later.
2. **Stamp the schema.** In the same transaction as the first `gitdrop`, insert `schema_migrations(version = 2)`.
   Builds from PR A on then see the boundary named; earlier builds keep failing closed on the record. **This cannot
   protect builds that predate the schema check**, which do not look at it, so the runtime guarantee for them is
   "refuse to open", not "refuse to write". If that is not acceptable, PR B must not be enabled.
3. **Compare-and-set.** `--expect-registered-ms`, an explicit confirmation, checked read-only and again under the
   writer lock (as `repo-update` does).
4. **Refuse while held or selected Git events exist** for the application; never auto-dismiss or replay them.
5. **No Docker, no Keychain.** Removal appends one record. It never touches a container, image, volume, route, or
   secret.
6. **Desktop**: **Remove registration…** in Settings → Source, with a confirmation that says what is and is not
   removed; **Remove application…** stays disabled.
7. Tests for each of the above, plus a removal racing a deployment, a poll and an edit.

## Not covered, on purpose

* Deleting saved credentials. They stay in the Keychain (a global service, shared by runtimes), bound to the name and
  to the removed registration's destination. A credential lifecycle is a separate design.
* Deleting an application.
