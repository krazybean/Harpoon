# Docker Integration

Harpoon exposes a Docker-compatible API via `unix:///tmp/harpoon-docker.sock` (0600, Unix-socket-only, no TCP). M8 adds standards-based Docker context integration so `docker --context harpoon` works without `DOCKER_HOST`.

## Endpoint

- Context name: `harpoon`
- Endpoint: `unix:///tmp/harpoon-docker.sock`
- Created via `docker context create harpoon --docker host=unix:///tmp/harpoon-docker.sock --description Harpoon`
- Harpoon never overwrites a foreign `harpoon` context; setup reports conflict and exits 1.

## CLI

```
harpoon docker setup   # create/verify context (idempotent)
harpoon docker status  # Docker CLI, context, endpoint, current context, runtime, socket
harpoon docker remove  # remove only if owned (endpoint matches)
harpoon docker use     # docker context use harpoon (explicit activation)
```

`harpoon docker setup` prefers `docker context create`; never manually writes `~/.docker`.

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

Existing contexts (`default`, `desktop-linux`) untouched. `harpoon docker setup` is idempotent; second invocation reports valid. `remove` refuses foreign context.

## Build

`docker --context harpoon build` and `docker --context harpoon buildx build --load` work via standard context; Harpoon provides BuildKit `v0.23.2` linux/arm64. No custom BuildKit integration.

## Security

Socket remains `0600`; integration never adds TCP. No network exposure.

## Failure

- Docker not installed → setup/status reports, exit 1
- Context conflict → setup reports, exit 1, no overwrite
- Stopped Harpoon → `docker --context harpoon version` fails normally (socket unavailable)
- Duplicate setup → idempotent success
- Remove absent → success; remove foreign → refuse
