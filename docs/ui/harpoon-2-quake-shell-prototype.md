# Harpoon 2 Quake Shell prototype

Status: **BOUNDED UX PASS / LIVE ACCEPTANCE PENDING** (2026-10-17 America/Chicago)

The Quake Shell ships globally in every build (decided 2026-10-18): one
slide-down console with a target dropdown that runs commands in a single
selected running container, without creating a second PTY owner. The
selection is tracked by full container ID, so a refresh that reorders or
drops containers never moves input to a different container; if the selected
container stops, the panel says so instead of falling through to another. `Ctrl+\`` toggles
the panel, `Escape` closes it and restores focus to the trigger, and target
identity remains visible as engine/context, full container ID prefix,
executable, and effective user. The fixture includes a stale target that is
explicitly rejected rather than allowed to receive input.

With a live container shelf, the panel now attaches to the same native PTY
owner used by Container Shell and CSSH. Hiding the panel detaches its xterm
consumer without closing the session; reopening reattaches and replays a
bounded output buffer. The explicit Close session action is the frontend close
path. Production target selection is limited to currently running containers;
there is no fixture shell fallback. The UX pass uses a 420px default panel
(capped below the viewport), a flexible terminal row, a keyboard-adjustable
height control, overflow-safe metadata, a visible focus path through native
controls, and a reduced-motion fallback.

## Native integration boundary

`shellSessionStore.ts` owns one session per explicit `(context, container ID,
executable, user)` identity and scopes native events by session ID. `ShellTab`
and Quake attach/detach consumers to that owner, so navigation and panel
hiding do not call `close_shell_session`. The backend transport remains the
single Docker/PTy execution path; CSSH still uses independent target keys.

Do not add the planned 10-second connect timeout or 15-minute idle expiry here.
Those are lifecycle policy changes and need evidence from the existing session
manager before being approved. Closing the Quake panel must only hide it; the
future shared owner must decide when a session is explicitly closed.

## Acceptance still pending

- Native Tauri/WebView visual acceptance at 1000×700 and 760px widths:
  header separation, terminal padding, no viewport clipping, launcher placement,
  Ctrl+backtick, Escape focus restoration, and reduced motion.
- Live round-trip confirmation that hiding/reopening keeps one native
  `open_shell_session` owner, preserves bounded output, and does not stop the
  container; explicit Close session must close only the PTY child.
- Real xterm attach/detach, PTY resize, disconnect/normal-exit distinction,
  and safe cleanup under the native WebView.
