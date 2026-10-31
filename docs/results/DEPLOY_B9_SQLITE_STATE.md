# Deployment B9 — SQLite state slice

Status: IN PROGRESS — first usable offline slice

This slice replaces the CLI's active deployment store with `deployments.sqlite3` while preserving the
existing engine `Journal` contract and replay codecs. The legacy `journal.wal` is imported once, left
untouched as an exportable recovery artifact, and is not used for subsequent reads or writes.

Implemented:

- versioned `schema_migrations` table and ordered `records` table;
- full SQLite transactions with `synchronous=FULL` before a write is acknowledged;
- exclusive owner-only writer lock for the runtime directory;
- deterministic replay through the existing deployment and resource-ledger codecs;
- fail-closed malformed-state errors and one-time legacy journal migration;
- startup recovery that marks in-flight deployments `interrupted` without releasing leases or
  touching Docker resources.

Offline evidence:

```text
cargo test --manifest-path deploy/Cargo.toml                         PASS (all engine tests)
cargo test --manifest-path deploy-journal/Cargo.toml                 PASS (26 legacy + 2 SQLite tests)
cargo check --manifest-path deploy-cli/Cargo.toml                    PASS
cargo clippy --manifest-path deploy-cli/Cargo.toml --all-targets -- -D warnings PASS
cargo clippy --manifest-path deploy-journal/Cargo.toml --all-targets -- -D warnings PASS
```

Not claimed by this slice: health-gated replacement, cutover recovery, rollback, live Docker
validation, or opening the shipping gate. Those remain blocked until the permanent-state milestone
is complete and its remaining acceptance evidence exists.

## CI follow-up (PR #124)

- `deploy-journal.yml` now permits the word `sqlite` only in `src/sqlite.rs` (and the re-export in `lib.rs`). A `rusqlite` crate, `std::net`, `std::process`, `UnixStream`, `tokio` and Docker references remain banned everywhere in the crate.
- The CLI integration tests run against `deployments.sqlite3` / `sqlite.lock`. Legacy-WAL tests that validate import compatibility were kept and now build a legacy-only runtime by hand.
- Defects found and fixed while migrating:
  - Read-only commands (`status`, `doctor`, `plan`) failed on a runtime that still had only `journal.wal`; they now read the legacy journal without creating anything.
  - The legacy import committed row by row, so an interruption could leave a partial SQLite store that was never completed; it is now one transaction.
  - `deployments.sqlite3` was created with the process umask; it is now created 0600 like the old journal.
  - `status` and `doctor` reported the writer lock from the legacy `journal.lock`; they now read `sqlite.lock` (falling back to the legacy lock).
  - `export-journal` on a legacy-only runtime reported no journal; it now exports `journal.wal`.
  - Open failures again carry the fail-closed, export-and-do-not-delete guidance.
