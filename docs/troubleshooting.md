# Troubleshooting

## Quick Diagnostics

```
harpoon status
harpoon doctor
harpoon logs --lines 100
harpoon docker status
```

## Common Cases

**Harpoon not running**
```
harpoon status  # → stopped
harpoon start   # → PID, socket
harpoon logs --lines 100  # check HOST_VZ_START_FAILURE etc.
```

**Duplicate start**
```
harpoon start  # → HARPOON_ALREADY_RUNNING (PID 123) exit 10
```
No socket unlink, no restart. Use `harpoon status` .

**Invalid resource**
```
harpoon config set memory 128  # → memory must be 512|768|1024
harpoon start --memory 128     # → FAILED validation exit 5
```
Check `harpoon config show` and `harpoon doctor`.

**Malformed config**
```
Harpoon configuration is invalid: <reason>
Config: ~/Library/Application Support/Harpoon/config.json
Hint: run harpoon config reset all or fix JSON
```
`doctor` also reports.

**Docker context missing**
```
harpoon docker status  # → not installed
harpoon docker setup   # → Creates harpoon → unix:///tmp/harpoon-docker.sock
```
`harpoon start` hints `harpoon docker setup` if missing.

**Docker context conflict**
```
Conflict: context 'harpoon' already exists but targets unix:///tmp/fake.sock
Refusing to overwrite.
```
`harpoon docker remove` only removes owned context (endpoint matches). Otherwise `docker context rm harpoon` manually.

**Socket permission**
```
harpoon doctor  # → FAIL socket 0600
```
Harpoon always creates `0600`. Check `ls -l /tmp/harpoon-docker.sock`.

**Port collision**
```
docker run -p 18080:80 ...  # → address already in use
```
Another service uses 18080. Use different host port.

**Unsupported bind**
```
/etc:/workspace  # → 500 host path not shared
```
Only `/Users` and `/tmp` (→ `/private/tmp`) are shared via VirtioFS.

**Stale PID**
```
Harpoon: stale PID 99999
```
Dead pid file. `harpoon start` cleans and recovers; `harpoon status` reports.

**Harpoon restart persistence**
`harpoon restart` preserves config; `restart --memory 1024` overrides this run only, not persistent config.

## Logs

```
harpoon logs
harpoon logs --lines 100
harpoon logs --follow
harpoon logs --path  # resolved path
```

Stopped Harpoon still has previous log (`harpoon.log.1`).

## Exit Codes

- `0` success
- `1` general/user error (validation, config, docker)
- `2` unknown command
- `5` config validation (memory/cpus)
- `7` VM start failure (VZErrorDomain 1)
- `10` already running

Duplicate `10` is stable for scripts.

## Known Limitations

- Explicit `127.0.0.1` HostIp mapping deferred (0.0.0.0 → 127.0.0.1 safe)
- UDP publishing deferred
- host→guest inotify not guaranteed (use polling)
- Fixed 2 GiB guest disk

See `harpoon doctor` for host checks.
**Host VZErrorDomain 1 transient**
```
HARPOON_STATE BOOTING -> FAILED reason=VM start failure VZErrorDomain 1 Internal Virtualization error
```
Host transient (also fails for repo bin + spike2 disk). Retry `harpoon stop; sleep 3; harpoon start`. If persists, host reboot/recovery needed. Not a packaging bug. Tracked as external blocker in M12.

**Installation boundary**
```
cd /tmp && harpoon doctor  # must PASS kernel/initramfs/disk via staged lib, not spike1/cache
```
If doctor shows repo paths when using staged bin, check `which harpoon` and `installedLibDir` (`/usr/local/lib/harpoon` or `bin/../lib/harpoon`).

**Provisioning truncated (36M bug, fixed)**
`du -m /tmp/harpoon-runtime/data/harpoon-root.img` should be ~962M, not 36M. Fixed via `cp -c` clone in M11.

