# Desktop shell transport

The Tauri backend owns one shell session per `open_shell_session` call. It
creates a macOS PTY and runs the resolved Docker CLI directly with:

```text
docker --context <context> exec -it <full-container-id> /bin/sh
```

The context and container ID are validated before process creation; no shell
interpolation is used. Input is bounded to 64 KiB per call and output is sent
as bounded `shell-event` chunks. Sessions are independent and are terminated
when explicitly closed or when the Tauri app exits.

Invoke `open_shell_session` with `{ context, containerId, cols, rows }`. Keep
the returned `sessionId` for subsequent calls:

- `write_shell_input` — `{ sessionId, input }`, bounded to 64 KiB per call.
- `resize_shell_session` — `{ sessionId, cols, rows }`, clamped by the backend.
- `close_shell_session` — `{ sessionId }`, terminates only that child session.

Listen for the Tauri `shell-event` event. Events are bounded output chunks of
the form:

```json
{ "sessionId": "shell-…", "kind": "output", "data": "…" }
{ "sessionId": "shell-…", "kind": "exit", "code": 0 }
```

The frontend should unsubscribe and close the session when its shell view is
destroyed. This transport is backend-only; no shell UI or live container
stress test is part of this seam.
