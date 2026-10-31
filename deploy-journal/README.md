# harpoon-deploy-journal

Durable, crash-recoverable journal for the deploy engine's first slice (Milestone B5). One append-only,
line-oriented file per journal directory; local files only. The line format and replay logic are pure and
live in `harpoon-deploy` (`journal_format`); this crate adds the file operations.

```text
<journal dir>/
  journal.lock    exclusive flock, held while a writer is open (never deleted); created 0600
  journal.wal     header line, then deployment snapshots and ledger records in the order they were made
```

* `FileJournal::open(dir)` — lock, read, cut off a torn tail, return what the file holds.
* `FileJournal::open_recovered(dir, now_ms)` — the same, then mark incomplete deployments `Interrupted` (persisted once).
* `read_only(dir)` — look without locking or writing.
* `impl Journal for FileJournal` — what the engine calls; `Ok` means appended **and** synced.

Pass `Restored::ledger` to the engine as `RunConfig::prior_ledger` so leases and unsettled entries of earlier
deployments are visible to port allocation. Recovery never redeploys, removes, or adopts anything.

Full contract, failure semantics, tests and unresolved risks: `docs/results/DEPLOY_JOURNAL.md`.

## One canonical directory
The journal directory is the unit of lease coordination: two writers are excluded only if they open the **same**
directory. The B6 CLI (and any other front end of the local runtime) must therefore use a single canonical, private
(`0700`) journal directory for that runtime. A second, independent journal directory would have its own lock and its
own ledger and so bypass cross-deployment lease coordination. `journal.wal` and `journal.lock` are created `0600`;
the crate does not create or chmod the directory itself, and does not tighten files that already exist.
