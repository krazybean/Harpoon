# Bootstrap event-push experiment

This is a reversible development-only experiment for the startup status animation.
It is intentionally disabled by default and is not an accepted product path.

Run the desktop UI with `?bootstrap-events` to move startup status observation and
the existing `harpoon start` invocation into the Rust bootstrap worker. The worker
emits `bootstrap-status` from fresh `status --json` observations and
`bootstrap-complete` when the authoritative state is ready or failed. The default
launch path is unchanged unless that query parameter is present in a development
build. Add `&bootstrap-trace` for minimal browser-console timing samples:
backend observation timestamp, event receipt, React replacement, and CSS animation
start.

The exact rollback is to remove `?bootstrap-events` from the URL. For a code-level
rollback, revert only `src-tauri/src/bootstrap.rs`, the `bootstrap` module and
commands in `src-tauri/src/main.rs`, and the event-gated additions in `src/App.tsx`;
the existing frontend bootstrap state machine remains intact underneath.

This experiment does not change the Harpoon runtime, VM, Docker, or container
lifecycle implementation. It only moves the already-authorized startup command
and status observation to a background Tauri thread. Native A/B acceptance was
not obtained; no visual success is claimed. The bounded stop decision is to
retain the stable default path and avoid further work unless a materially
simpler approach appears.
