# Compose

Harpoon runs standard Docker Compose without a custom implementation. All Compose operations go through the Docker API over `unix:///tmp/harpoon-docker.sock` via `docker --context harpoon`.

## Preferred Workflow

```
harpoon start
harpoon docker setup

docker --context harpoon compose up -d --build
docker --context harpoon compose ps
docker --context harpoon compose logs
docker --context harpoon compose down
```

Optional activation:

```
harpoon docker use
docker compose up -d
```

Harpoon never auto-switches context; `harpoon start` only hints.

## Fixture

`harpoon/fixtures/m9-compose` (`harpoon-m9-test`):

- `app` (alpine:3.22, build, `18080:3000`, bind `./src:/app/src:rw` + `src-ro:/app/ro:ro`, env, env_file, depends_on `postgres` healthy, networks `m9net`, mem_limit 64m, restart unless-stopped)
- `postgres` (postgres:15-alpine, `18081:5432`, pgdata volume, healthcheck `pg_isready`)
- `redis` (redis:alpine)
- `worker` (alpine:3.22, scale)

All bind sources under `/Users/.../Harpoon` → `HostPathTranslator` → `/mnt/harpoon-host/Users`.

## Verified

- `compose config` canonicalizes to absolute `/Users/...` and is translated
- `compose build` and `up -d --build` succeed
- `create/start/stop/restart/down` lifecycle with dynamic port reconciliation (M5)
- Bind `host→container`, `container→host`, `ro` rejects
- Named volume `pgdata` persists across `down` and survives `harpoon stop/start`
- Networks: `m9net` bridge, `app → postgres`/`redis` via `getent hosts` + `nc -z`
- Ports `18080:3000`, `18081:5432` from macOS `curl`/`nc`, multiple simultaneous, stop removes, start restores
- Env: `environment` + `env_file` + `${VAR}` interpolation
- Healthcheck: `postgres` `pg_isready`, `app` `wget /health`, `depends_on: service_healthy`
- Logs/exec/ps via existing hijack bridge
- Scale `worker=3` then `1` with DNS stable
- Project naming `harpoon-m9-test` scoped
- Harpoon restart persistence: `m9harpoon` table survives `harpoon stop/start`
- Failures: invalid image, port collision, unsupported `/etc:/workspace` → `500 host path not shared`
- Resource limit `mem_limit: 64m` → `HostConfig.Memory 67108864`
- Watch: `compose watch` available (polling), not required
- DOCKER_HOST legacy still works

## Known Limitations

- Explicit `127.0.0.1` HostIp mapping still deferred (0.0.0.0 → 127.0.0.1 safe)
- UDP not published
- host→guest inotify not guaranteed (polling works, watch via sync if polling)
- Fixed 2 GiB guest disk
