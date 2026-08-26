# Lifecycle

Harpoon M7 turns the Phase 1 foreground VM process into a background-managed service.

## Process Model

Before M7, `harpoon` *was* the VM runtime: running it held the foreground RunLoop, VM, vsock bridge, and `/tmp/harpoon.lock`.

After M7:

- **One Harpoon runtime process owns**: `VZVirtualMachine`, `/tmp/harpoon.lock`, `/tmp/harpoon-docker.sock`, `/tmp/harpoon-control`, port forwards, lifecycle state.
- **CLI commands manage that process**: `harpoon start/stop/status/logs` are thin managers. They do not duplicate VM ownership.

```
CLI  ──► harpoon start ──► Process("harpoon run") ──► VZVirtualMachine
                ▲                         │
                │ status/stop/logs        └── /tmp/harpoon.lock (flock)
                │                         └── /tmp/harpoon-docker.sock 0600
                                          └── ~/Library/Application Support/Harpoon/harpoon.log
```

Foreground mode remains for debugging: `harpoon run`.

## CLI

```
harpoon start [--cpus N] [--memory 512|768|1024] [--kernel PATH] [--initramfs PATH] [--disk PATH]
harpoon stop
harpoon status
harpoon logs [--follow] [--lines N]
harpoon restart [--cpus ...]
harpoon run [--cpus ...]   # foreground/debug, also: bare `harpoon [--flags]` for Phase 1 compat
harpoon version
harpoon help
```

`harpoon start` prints:

```
Docker socket: /tmp/harpoon-docker.sock
export DOCKER_HOST=unix:///tmp/harpoon-docker.sock
```

It does not mutate shell env.

## Background Start

`harpoon start` does:

1. Check already-running via `flock(/tmp/harpoon.lock)` probe and PID liveness. If held, prints `HARPOON_ALREADY_RUNNING` and exits 10 without unlinking existing sockets.
2. Ensure runtime dir, rotate `harpoon.log` → `harpoon.log.1`.
3. Spawn `harpoon run` via `Process` with stdin = nullDevice, stdout/stderr → `harpoon.log`, child survives terminal exit (orphaned to launchd).
4. Write `runtime.pid` and `runtime.json` (pid, startedAt, cpus, memoryMiB, diskPath, socketPath, uuid, binary).
5. Poll up to 60s for `HARPOON_RUNNING` in log + socket exists. On success print running info; on failure print tail, clean pid/json, return nonzero, keep log.
6. If log contains `HARPOON_ALREADY_RUNNING` or `HOST_VZ_START_FAILURE`, fail fast.

Resource precedence: CLI > env (`HARPOON_CPUS`, `HARPOON_MEMORY_MIB`) > defaults (2 cpus, 1024 MiB). `harpoon run` uses same parsing.

## PID / State Files

Preferred durable location:

```
~/Library/Application Support/Harpoon/
  runtime.pid      # pid of background runtime
  runtime.json     # {pid, startedAt, cpus, memoryMiB, diskPath, socketPath, uuid, binary}
  harpoon.log      # current run log (stderr)
  harpoon.log.1    # previous run log (rotation on start)
```

Ephemeral:

```
/tmp/harpoon.lock          # flock single-instance
/tmp/harpoon-docker.sock   # 0600 docker bridge
/tmp/harpoon-control       # 0600 balloon control
/tmp/harpoon-stop          # sandbox fallback stop signal
```

Fallback: if Application Support not writable (e.g., Muse sandbox), runtime dir falls back to `/tmp/harpoon-runtime` with same files. This keeps tests runnable under sandbox while production uses Application Support.

Metadata is not authoritative; live state (process liveness, lock, socket, Docker API) wins.

## Status

`harpoon status` checks live state, not just pid file:

1. pid file exists + `kill(pid,0)` + `proc_pidpath` harpoon check
2. `/tmp/harpoon.lock` flock probe
3. socket exists + `connect()` or `HARPOON_RUNNING` in log (sandbox fallback)
4. optional Docker API via socket

States:

- `running`: pid alive + is harpoon + socket + (dockerReady or log has HARPOON_RUNNING)
- `starting`: pid alive + harpoon but socket not ready
- `degraded`: pid alive but socket/lock inconsistent
- `stale`: pid file points to dead pid or non-harpoon pid (PID reuse)
- `stopped`: no pid, no lock, no socket

Stale pid is optionally cleaned (pid dead + no lock/socket). `harpoon status` never mutates running state.

## Stop

`harpoon stop`:

1. Read `runtime.pid`, verify `isHarpoonProcess` via `proc_pidpath` (PID safety, no blind kill).
2. `kill(pid, SIGTERM)` + create `/tmp/harpoon-stop` fallback (poll every 1s in runtime).
3. Wait ≤10s for process exit, sockets removed, lock released.
4. On success remove `runtime.pid`/`runtime.json`/`/tmp/harpoon-stop`, print `Harpoon stopped`. On timeout report and do not SIGKILL (user must diagnose).

Never kills VZ XPC directly.

## PID Safety

Do not blindly signal pid from file. Verify via `proc_pidpath` that executable path contains `harpoon`. If pid alive but not harpoon (reuse), report `stale` and refuse to signal. UUID/start time in json is available for future strictness.

## Logs

`harpoon logs` prints `harpoon.log`. Options:

- `--lines N` / `-n N` / `--lines=N`: tail N lines
- `--follow` / `-f`: `tail -F` the log

`harpoon run` still prints to terminal (stderr). Rotation is single-file: previous `harpoon.log` moved to `harpoon.log.1` on start.

## Duplicate Start & Stale Recovery

- Second `harpoon start` while running: `HARPOON_ALREADY_RUNNING`, exit 10, first sockets untouched, `docker version` still succeeds (verified via `isSocketLive` and owned-only removal).
- Stale pid (dead process): `status` reports `stale`, `start` removes stale files and recovers.
- PID reuse (pid now unrelated): `status` reports `stale`, `stop` refuses to signal, `start` removes stale and recovers.

## Terminal Independence

`harpoon start` spawns via `Process` with detached stdio; parent exits after readiness, child is orphaned. Closing invoking terminal does not kill runtime. Verified by `harpoon start; close terminal; harpoon status` still `running`.

## Failure Semantics

Invalid `--memory`/`--cpus` or missing kernel: child fails before bridges, parent sees `FAILED` state, removes `runtime.pid`/`runtime.json`, no socket, no lock, log retained, returns nonzero. No stale files.


## Docker Context (M8)

- Context `harpoon` → `unix:///tmp/harpoon-docker.sock` via `docker context create`
- `harpoon docker setup/status/remove/use` (see [Docker Integration](docker-integration.md))
- `harpoon start` hints `docker --context harpoon ps`, does not auto-switch
- Context survives stop/start; `docker --context harpoon` works without `DOCKER_HOST`

## Known Limitations (M7)

- Host transient `VZErrorDomain Code=1` can block `HARPOON_RUNNING` until reboot; `harpoon start` then returns nonzero with log tail and cleans pid.
- `~/Library/Application Support/Harpoon` may be unavailable under sandbox; fallback to `/tmp/harpoon-runtime` is used for tests.
- `harpoon logs --follow` uses `tail -F`.
