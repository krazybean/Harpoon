# Docker Integration

Harpoon exposes a Docker-compatible API via `unix:///tmp/harpoon-docker.sock` (0600, Unix-socket-only, no TCP). Its standard `harpoon` context lets `docker --context harpoon` work without `DOCKER_HOST`. The current packaged testing prerelease is [v2.0.0-rc.4](releases/v2.0.0-rc.4.md); v0.1.1 remains the latest stable desktop release.

## Endpoint

- Context name: `harpoon`
- Endpoint: `unix:///tmp/harpoon-docker.sock`
- Created via `docker context create harpoon --docker host=unix:///tmp/harpoon-docker.sock --description Harpoon`
- `harpoon docker setup` **repairs** an existing `harpoon` context pointing at the wrong endpoint (tries `context update`, then remove/recreate if necessary). It does not change the default Docker context. Be aware that a preexisting context *named* `harpoon` may therefore be changed; inspect it first if you use that name for another engine.

## CLI

```
harpoon docker setup   # create/verify context (idempotent)
harpoon docker status  # Docker CLI, context, endpoint, current context, runtime, socket
harpoon docker remove  # remove only if owned (endpoint matches)
harpoon docker use     # docker context use harpoon (explicit activation)
```

`harpoon docker setup` verifies an existing correct context; repairs an incorrect one using the Docker CLI; or creates it if absent. It never manually writes `~/.docker`.

## Workflow

```
harpoon start
harpoon docker setup
docker --context harpoon ps
docker --context harpoon run --rm hello-world
docker --context harpoon build -t myimage .
```

Optional activation:

```
harpoon docker use
docker ps   # now uses harpoon without --context
```

`harpoon start` does NOT change active context; it prints:

```
Docker context: harpoon
  docker --context harpoon ps
```

or hint `harpoon docker setup` if missing.

## Precedence

Docker CLI precedence: `--context` flag > `DOCKER_HOST` env > current context. Verified: `DOCKER_HOST=unix:///var/run/docker.sock docker --context harpoon version` still talks to Harpoon (harpoon context wins). Legacy `export DOCKER_HOST=unix:///tmp/harpoon-docker.sock` remains supported.

## Lifecycle

Context survives `harpoon stop`/`start`. When stopped, `docker --context harpoon version` fails (socket unavailable); after `start` same context works without recreation.

## Coexistence

Existing contexts with other names (`default`, `desktop-linux`) remain untouched; `harpoon docker setup` may repair only the context named `harpoon`. It does **not** select a different active/default context. `remove` refuses a context whose endpoint is not Harpoon's.

## Build

`docker --context harpoon build` and `docker --context harpoon buildx build --load` work via standard context; Harpoon provides BuildKit `v0.23.2` linux/arm64. No custom BuildKit integration.

## Security

Socket remains `0600`; integration never adds TCP. No network exposure.

## Failure

- Docker not installed → setup/status reports, exit 1
- Context named `harpoon` with a wrong endpoint → setup tries to repair it, returning an error only if repair fails; do not use this name for a different engine.
- Stopped Harpoon → `docker --context harpoon version` fails normally (socket unavailable)
- Duplicate setup → idempotent success
- Remove absent → success; remove foreign → refuse
