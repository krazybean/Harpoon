# Milestone B5 — Durable journal (JSON-lines-style, first slice only)

Status: on `feature/deploy-b5-journal`; **not merged**. No Docker, Buildx, VM, runtime, UI, release or existing workload was
touched. SQLite remains the permanent store before real release replacement or rollback; nothing here is a database layer.

## What exists
| Where | What |
|---|---|
| `deploy/src/journal_format.rs` (pure) | Versioned deployment-snapshot codec (`snap\t1\t…`), file header (`header\tharpoon-journal\t1`), shared line framing, whole-file `recover` (torn tail vs corruption), `replay` into deployments + ledger. The ledger codec from B1 is reused unchanged. |
| `deploy/src/ledger.rs` | The ledger now refuses a port lease another deployment still holds (`LedgerError::LeaseHeld`), on live writes **and** on replay. |
| `deploy/src/engine.rs` | `RunConfig::prior_ledger` (a run starts from the restored ledger); `Report::deployment`, and `cleanup_problems()` is scoped to the report's own deployment. |
| `deploy-journal/` (new crate, file I/O) | `FileJournal` (implements the engine's `Journal`), `open` / `open_recovered` / `read_only`, `Wal` seam for fault injection. |

## The file
One append-only file per directory (`journal.jsonl`) plus `journal.lock`. Snapshots and ledger records share the file in the order they were
made, so the order between an intent and a snapshot survives a crash. Every line is `<crc32>\t<body>\n`. **Note:** lines are
checksummed tab-separated text (the B1 ledger codec), not JSON objects; "JSON lines" is honoured as *one self-contained record per line,
append-only*. Moving to literal JSON would need a JSON reader in the dependency-free `deploy` crate and is a format-version bump (the header exists for that).

## Guarantees
1. **Durability before side effects.** `persist`/`record` return `Ok` only after the whole line was appended in one `write` and the file was
   `sync_all`'d. A new file's directory entry is synced before the header is written (and again on any open that finds no header). Tests:
   strict `append, sync` alternation, `sync_dir` before the first byte, and a sweep proving every object that exists in the runtime when a
   run stops has its intent already in the file.
2. **Torn tail vs corruption.** A final line without a newline is a write that was never acknowledged: it is cut off (file truncated and synced)
   before any append, so the next record is never glued to a fragment. A newline-terminated line that fails its checksum or does not decode
   is **corruption**: `open` returns an error naming the line, the file is not modified, the lock is released, nothing after it is trusted.
   This includes a damaged *last* record. A foreign file (unterminated, not a prefix of the header) is `NotAJournal` and never truncated.
3. **Failed writes stop the engine.** After a failed append the journal tries once to cut the line back, then refuses all further writes
   (`broken`). After a failed `fsync` it refuses further writes and never retries the sync (the state of the file is unknown). The engine
   sees `EngineError::Journal(Io)` and stops; nothing it did afterwards depends on an unrecorded write. Reopening recovers from what is on disk.
4. **One writer.** An exclusive, non-blocking `flock` on `journal.lock`, held for the life of the handle. A second opener — another process or
   another handle in this one — gets `OpenError::Locked` immediately: no waiting, no stealing, no PID file. The kernel drops the lock when the
   holder dies, so a killed writer leaves no stale lock (tested with a real child process aborted mid-run). The lock file is never deleted.
   Lock is taken **before** the journal is read, so what a writer reads is what it extends. `read_only` takes no lock and never writes.
5. **No duplicate leases.** Because there is one writer per journal and the ledger refuses a held port at `apply` time (before the journal write),
   two deployments cannot both hold a port. A run started with `prior_ledger` also *allocates* around journaled leases (`Occupancy::with_ledger`).
6. **Conservative recovery.** `open_recovered` marks every non-terminal deployment `Interrupted` (persisted once; a begun, unconfirmed cutover is
   listed in `startup.incomplete_cutovers`) and does **nothing else**: no cleanup records, no removals, no adoption, no redeploy, no runtime
   call. The runtime is not even reachable from this crate. Ledger entries and their leases stay held until a caller that can verify the
   runtime runs the existing `reconcile` (an explicit step; tested against the fake runtime with bystanders untouched). Repeated recovery writes nothing.
7. **What a restart learns** (`Restored`): latest snapshot per deployment (status, failure, cutover, recovery, interruption, steps), the ledger
   (resource ids from receipts, cleanup failures/refusals/interrupted attempts, unsettled entries), outstanding leases, bytes discarded.

## Tests (real files in temporary directories)
`deploy`: 135 (124 + 9 format + 2 engine hooks). `deploy-docker`: 113, unchanged. `deploy-journal`: 24. Clippy `-D warnings` and fmt clean in all three.
- **Crash at every persistence boundary:** 5 scenarios × every journal write × 5 failure shapes (nothing written, 1 byte, half, all but the newline,
  whole line written but unacknowledged) = >300 cases, each followed by restart → recovery → explicit reconcile → invariants (no unresolved
  intent, nothing of the deployment left unaccounted for, bystanders untouched, file equals the process's belief) → a second restart that must
  write nothing.
- **Truncated records:** the journal cut at every line boundary ±1 and at every byte of the last two records, both as pure parsing (every byte of a
  sample) and through real `open` (prefix restored, tail removed, next append clean).
- **Checksum/corruption:** a flipped byte in each record in turn (line number reported, file untouched, lock released, twice); damaged last record;
  every single-byte change to a line is detected; bad header, second header, unknown file/snapshot version, contradictory records, foreign files.
- **Failed writes:** failed append (no further attempt), partial write healed / left for the next open, failed fsync (not retried, journal poisoned, record may or may not be present), failed directory sync (nothing written).
- **Permissions:** missing directory, read-only directory (nothing left behind), read-only journal, unreadable journal, torn tail that cannot be removed.
- **Concurrency:** second opener refused with nothing written; six threads taking turns writing 90 records (each exactly once, in order); a real child process refused while we hold the lock; a real child aborted while holding it, after which the next open succeeds and sees every acknowledged record.
- **Restart:** field-for-field restoration of a failed deployment with a failed cleanup (ids, `CleanupFailed`, leases); outstanding lease blocks the same port after restart, steers allocation, is refused by the ledger, and is only freed by an explicit reconcile.
- **Mutation checks:** 22 defects injected (append not synced; directory not synced; torn tail not cut; lock not taken; failed sync/append not poisoning; fsync retried; foreign file truncated; interruption not persisted / persisted on every open; failed append not cut back; header not written; broken journal still writing; lease check removed; prior ledger ignored; other deployments' problems charged to a report; terminal change allowed; snapshot/file version ignored; snapshot checksum unchecked; replay keeping the first snapshot; a bad last line treated as torn): **22 caught**. One survived the first pass (directory sync: the counter was incremented independently of the call) and led to moving the directory sync behind the `Wal` seam so the test observes the call.

## Unresolved risks
1. **Not literal JSON** (see above). Decide before B6 whether the permanent format should be JSON or whether SQLite replaces the file first.
2. **No power-loss testing.** Tests model process death and torn writes (prefixes). They assert `fsync`/directory sync are *called* in the right order; they cannot show the OS or disk honoured them (macOS needs `F_FULLFSYNC`, which Rust's `sync_all` uses there). A power cut that persists a later block but not an earlier one could leave a newline-terminated, garbled last line; that is reported as corruption (needs a human), not repaired.
3. **No tooling for a corrupt journal** beyond refusing to open it: no quarantine, salvage or manual-override command. Deployments cannot start from a directory whose journal is corrupt until someone moves the file.
4. **One deployment process per journal directory.** Contention fails fast; there is no queue or wait. `flock` is not reliable on network filesystems; the directory must be local. Unix only (flock, directory fsync).
5. **Leases of an interrupted deployment stay held** until reconciled against a verified runtime. A deployment that is never reconciled blocks its port indefinitely. Reconciliation itself needs the runtime (B6/B7 wiring).
6. **No rotation or compaction; whole file read on open.** Fine for the first slice; SQLite is the answer for longer-lived history.
7. **Not wired in.** Nothing in the shipping app calls `FileJournal`; `DockerBackend` already allocates from `io.ledger()`, so passing `Restored::ledger` as `prior_ledger` is all it takes. A broken handle ends the run; there is no in-process healing.
8. **Timestamps** in recovery come from the caller's clock; a skewed clock affects `Interrupted` times only.
9. **Report semantics changed:** `Report::cleanup_problems()`/`is_clean()` now consider only the report's own deployment; the full ledger (including other deployments' entries when `prior_ledger` is set) is still in `Report::ledger`.

## Gates for later milestones
B6 (not started): explicit developer-run reconciliation/recovery UX over `Restored`. B7 (not started): live-runtime verification, including reconciling interrupted deployments' real containers. SQLite before release replacement or rollback.
