# Shell transport prototype

Status: **OFFLINE MOCK PASS / LIVE VALIDATION DEFERRED** (2026-10-15 America/Chicago)

The bounded prototype in `tools/shell-transport-prototype.mjs` proves the proposed
boundary without starting Harpoon, Docker, a VM, or touching guest storage:

- one Docker control seam: `docker --context harpoon exec --interactive --tty <full-container-id> /bin/sh`;
- one independent session per container ID, allowing concurrent sessions;
- length-prefixed JSON frames with base64 terminal bytes so arbitrary PTY output is not confused with control messages;
- explicit `stdin`, `resize`, `cancel`, `data`, and `exit` frames;
- cancellation removes only the selected session;
- names and short IDs are rejected so stale UI context cannot silently target another container.

The prototype deliberately does not claim PTY allocation, Docker Engine hijack behavior,
window-size propagation, or process-tree cleanup on the live Harpoon socket. Those need
one disposable running fixture and must be validated through the existing Docker context,
not a second Docker API client.

Run the offline proof with:

```sh
node --test tools/shell-transport-prototype.test.mjs
```

Disposable-fixture validation plan: start Harpoon only with explicit authorization, create
one temporary container whose full ID is captured from `docker --context harpoon inspect`,
open two sessions, verify input/output and resize independently, stop the container,
disconnect the client, and confirm both sessions emit bounded exit/error frames and leave
no orphaned `docker exec` processes. Remove only that fixture after the evidence is saved.
