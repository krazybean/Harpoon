# Removing a Git registration

Removing a registration disconnects an application from its repository. It does not delete the application, its
deployments, its serving release, any Docker resource, or any saved credential. It is a different thing from
deleting an application, which this work does not touch.

This is delivered in two steps because the journal cannot be read by a build that does not know the record:

| Step | Contents | Writes a removal? |
|---|---|---|
| PR A (merged) | the `gitdrop` record, replay, snapshots, race protections, identity and credential rules, a journal-schema check | No |
| **PR B (this change)** | `repo-remove`, the confirmation dialog, the capability gate, the backup and schema stamp, poll incarnations | **Yes, but closed in every shipping build** |

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

## The writer: `repo-remove` (PR B)

`repo-remove --name N --expect-registered-ms T --confirm-removal` appends one `gitdrop` record. It never touches
Docker, a route, a deployment record, a source record, a held event, or the Keychain (the tests run it against a
Keychain that fails on any access), and it does not start, stop or replace anything. The registration disappears; the
application, its history, its serving release and its saved credentials stay.

### Disabled by default; how it is enabled (and what cannot enable it)

`Gate.git_removal_enabled` is **false in `Gate::shipping()`**, which is what the shipping CLI and the desktop use. Two
builds can open it, and only for a runtime that was authorized by hand:

| Build | How it gets the capability |
|---|---|
| `harpoon-deploy-acceptance` (the unpackaged acceptance binary) | its source sets the field; no other source may (a packaging test enforces it) |
| the desktop, **debug build with an active test namespace** (`HARPOON_TEST_TMPDIR`, which a release build ignores) | `operation_gate()` derives it; a release build has no namespace |

There is **no environment variable, setting, command or UI action** that enables removal, and the CLI sources may not
read the environment at all (a CI check).

Even in a capable build, `repo-remove` refuses unless *the runtime directory* carries its own enablement marker. The
person authorizing removal for that runtime creates it by hand, and no command writes it:

```sh
RT="$(cd /path/to/isolated/runtime && pwd -P)"          # the canonical path
printf 'harpoon-git-removal 1\nruntime=%s\n' "$RT" > "$RT/git-removal.enabled"
chmod 600 "$RT/git-removal.enabled"
```

### Capability is verified before the journal is touched

`repo-remove` checks, in this order, and stops at the first failure (exit 6, nothing read or written):

1. the build's gate is open;
2. the runtime directory is **not** an application data directory (anything containing `/Library/Application Support`,
   which includes the production data directory) and lives under a temporary directory (`/tmp`, `/private/tmp`, or
   the user's temporary directory);
3. the marker exists, is a regular file (not a link), is mode 600, is owned by the runtime directory's owner, is at most
   4 KiB, and names **this** runtime's canonical path with the exact first line `harpoon-git-removal 1`. A marker copied
   to another runtime names the wrong path and is refused;
4. then the request itself: the registration exists, `--expect-registered-ms` is the version shown, the removal is
   confirmed (`--confirm-removal`), and no held or selected Git event waits. This is done read-only first;
5. only then is the writer opened, and (4) is checked again under the writer lock.

Every refusal leaves the journal byte-identical (tests compare the file).

### Repeated and stale requests

A removal of a name that was already removed is refused ("was already removed"), as is one carrying an old version
("was changed after you opened it"). Two removals racing for one registration take turns on the writer lock; the second
finds it gone. No duplicate record and no second backup is ever made.

### The one-way schema transition and its backup

The first removal in a journal upgrades it from schema 1 to schema 2. In order:

1. the writer's exclusive lock is taken (`sqlite.lock`) and **held to the end**, so no other process can change the
   journal between the backup and the upgrade;
2. a free-space proof: as many bytes as the backup needs (twice the journal plus 1 MiB) are written and flushed in
   `<journal>/backups/`, then removed;
3. a consistent copy is made by SQLite itself (`VACUUM INTO`), from the writer's connection, outside any transaction, in
   `<journal>/backups/pre-schema2-<ms>.sqlite3` (the directory is mode 700 and owned by the journal's owner; the file is
   set to 600 and flushed, as is the directory);
4. the copy is **proved**: mode 600; SQLite's `integrity_check` says `ok`; it declares schema 1 and holds no `gitdrop`;
   its records equal the journal's records one for one; it replays to exactly the state the journal has; and the
   journal's own records did not change meanwhile;
5. its SHA-256 is computed and written beside it (`...sqlite3.sha256`, mode 600) and reported;
6. **in one SQLite transaction** the schema-2 row and the `gitdrop` record are written (`BEGIN IMMEDIATE ... COMMIT`),
   so there is never a `gitdrop` without the stamp, nor a stamp without a removal.

If any step before 6 fails, **nothing is written** and no partial backup is left. Later removals find schema 2 and only
append the record.

### Recovery (offline, explicit, never automatic)

Nothing in Harpoon restores a backup, downgrades a journal, or edits a backup. To go back to the schema-1 journal:

1. Quit every Harpoon process that opens the journal (the desktop, `harpoon-deploy`, a running `watch`). A clean quit
   matters: the journal uses SQLite's default rollback journal (`journal_mode=delete`), so there are **no `-wal` or
   `-shm` files**, but a process killed mid-write can leave a hot `deployments.sqlite3-journal`.
2. Verify the backup against its digest: `shasum -a 256 backups/pre-schema2-<ms>.sqlite3` must equal the first word of
   the `.sha256` file.
3. Keep the current file: `cp deployments.sqlite3 deployments.sqlite3.after-removal` (the removal is in it).
4. Copy the backup over the live file: `cp backups/pre-schema2-<ms>.sqlite3 deployments.sqlite3`, and **delete any
   `deployments.sqlite3-journal`, `-wal` and `-shm` next to it** so SQLite does not roll old pages into the restored file.
5. Run `repo-list` and `status`: the registration is back and the schema is 1. The restored file is the backup, not an
   edit of it; its `schema_migrations` already says 1, so no row is removed by hand.

**Restoring loses every journal change made after the backup was taken**: deployments, registrations, held events,
configuration and observations alike. Filesystem durability is as strong as the backup's `fsync` and the volume's: the
backup file and its directory are flushed before the removal is written, but a power loss can still lose what the disk
had not yet committed, which is why the digest is checked before a restore.

### Poll event identity (incarnations)

An application's **incarnation** is the number of `gitdrop` records for it, counted from the whole immutable
history. A poll's delivery id is `poll:<name>:<commit>` for incarnation 0 (a never-removed registration, unchanged),
and `poll:<name>:r<n>:<commit>` from the first removal on. An edit never changes it; each removal and explicit
reclaim does. Events already recorded keep their ids. Without this, a commit that an earlier incarnation had queued,
dismissed, failed or deployed would be silently dropped as a duplicate when the branch returned to it. (A reclaimed
registration still starts from a fresh polling baseline, so nothing already handled is queued again by the switch.)

### Desktop

**Remove registration…** sits in Settings, Source, next to Edit registration, separately from **Remove application…**
(still disabled, unchanged). It is disabled with the reason unless the build allows removal. The dialog states what is
removed and what is kept (including that saved credentials stay in the Keychain tied to the name and its address),
warns that the journal is upgraded one way and backed up first, refuses while events wait, and needs the repository's
name typed. After removal the application keeps its readable name and shows **Source disconnected**; Deploy, Redeploy
and Retry are disabled with the reason, and rollback is not.

## Older builds against an upgraded journal

Measured with the build that preceded PR A against a scratch journal holding a `gitdrop`: every command, reads and
writers alike, exited 4 ("the journal is not usable: invalid SQLite record: unknown record."), and the journal file was
byte-identical afterwards. Builds from PR A on read it (and name a newer schema precisely). So an older build fails
closed and cannot damage the journal, but cannot use it, and there is no downgrade except the backup above.

`tools/check-journal-readers.sh BINARY...` scans binaries for the reader (the `gitdrop` record and the schema message)
and fails for any that lacks it: run it over the installed `Harpoon.app` executable, any `harpoon-deploy` on the
`PATH`, the acceptance binary and any development builds before authorizing a removal on a journal they could open.

## Production

PR B enables nothing in production: the shipping gate is closed, the production data directory is refused by path
even for a capable build, and no marker exists there. Opening it for production is a separate, reviewed change
(like Gate U) that may be made only when (1) the reader check above passes for every build that can open the journal,
(2) a release containing PR A has been the minimum everywhere, and (3) the backup and recovery procedure has been
rehearsed on a copy. Builds that predate the schema check cannot be asked anything at runtime: for them the guarantee
is "refuse to open", not "refuse to write", and that limit is accepted only knowingly.

## Not covered, on purpose

* Deleting saved credentials. They stay in the Keychain (a global service, shared by runtimes), bound to the name and
  to the removed registration's destination. A credential lifecycle is a separate design.
* Deleting an application.
