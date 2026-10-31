# harpoon-deploy-cli

Gated developer CLI (`harpoon-deploy`) over the deploy engine, `DockerBackend` and the durable SQLite
state store (`deployments.sqlite3`). Existing first-slice `journal.wal` state is imported once.
Commands: `status`, `doctor`, `plan`, `recover`, `reconcile`, `deploy`, `export-journal`. Run `harpoon-deploy help`.

* Every command needs `--runtime-dir DIR` (absolute). One runtime directory = one canonical journal
  (`DIR/deploy-journal`, mode 0700; files 0600). There is no default and no environment fallback.
* Docker is contacted only through an explicit `--docker-socket PATH`; `DOCKER_HOST`, contexts and `~/.docker` are never read.
* Read-only commands never write, create, lock-for-write or mark anything.
* Live `deploy` is disabled in the shipping build (`Gate::shipping()`) until B7; `deploy --simulate` runs the engine's
  `FakeBackend` through the real journal in a separate *simulated* runtime directory.

Details, exit codes and the B7 hand-off: `docs/results/DEPLOY_CLI.md`.
