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
