# harpoon-deploy-docker — Docker backend (Milestone B3)

The first real `harpoon_deploy::backend::Backend`, for the **first disposable deployment only**.
It lives in its own crate so `deploy/` (the engine) stays offline: all socket and process I/O is here.

- Talks HTTP/1.1 to **one configured Unix socket** (`DockerConfig::socket`). It never reads
  `DOCKER_HOST`, Docker contexts or `~/.docker`.
- Builds only through the **bundled Buildx** (`harpoon_deploy::buildx::resolve`), run with a cleared
  environment plus `isolated_env`. A missing/unusable binary is an explicit error; there is no fallback.
- Create/start/stop/remove/inspect containers, inspect/remove images, list containers and images,
  read bounded logs, read Docker's published ports. The API module has **no** prune, pull, push,
  exec, network or volume call, and removal is never forced.
- Ledger protocol on every create: intent (durable) → create → receipt with Docker's id. Removal
  re-reads the object by id and checks id, name and all three Harpoon labels first.
- Health evidence comes from real HTTP observations of the published port (`probe::TcpHttp`); it
  is never `simulated`, and the engine refuses simulated evidence from a backend that doesn't accept it.
- The intended image tag must be unused (never overwritten); images that declare volumes are refused (no volume management).
- `stop_current` / `restore_previous` (release replacement) are deliberately not implemented.

Tests use a test-owned socket with a fake Docker (`tests/support/fake_docker.rs`) and a fake Buildx
shell script. Nothing contacts a real Docker endpoint. The fake's response shapes follow the Docker
Engine API documentation; they were **not captured from a live engine**.

`cargo fmt --check && cargo clippy --offline --all-targets -- -D warnings && cargo test --offline`
