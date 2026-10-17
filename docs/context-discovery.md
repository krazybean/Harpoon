# Harpoon 2 Context & Engine Discovery

Status: bounded read-only prototype implemented; native live acceptance pending.

The desktop `Contexts` view calls the Rust `discover_contexts` command only as
an asynchronous refresh. It reads Docker context metadata/current selection,
the relevant `DOCKER_HOST` signal, conventional Docker/Podman Unix socket
candidates, and Podman system connections. Each bounded CLI probe has a
two-second ceiling, uses no shell, receives no credentials, and sends no
mutating command. Endpoint identities are normalized and deduplicated.

States are deliberately separate from selection: `detected`, `connected`,
`unavailable`, and `authentication_needed`. A connected result exposes only
read/inspect capability availability; mutation is explicitly `unsupported`.
Missing executables, dead sockets, duplicate endpoints, authentication errors,
and partial fixture data are covered by offline Rust tests. The UI makes the
read-only boundary visible and does not offer context switching or connection
configuration.

## Explicit deferral

The proposed future AP-controlled backend for authorized container commands
(build, restart, storage, and networking) is not part of this prototype. It is
a separate, high-risk feature requiring explicit authorization, scoped
permissions, auditability, target validation, and failure/cancellation policy.
No AP integration, generalized runtime adapter, or container operation API is
implemented here.

## Acceptance path

1. Run the focused fixture tests and frontend build offline.
2. On a healthy macOS host, review Docker Desktop, Harpoon, Podman/rootless,
   stale-socket, duplicate-endpoint, and authentication-required cases.
3. Verify with filesystem/process observation that discovery never switches a
   context, starts a runtime, changes a socket/config, or prompts for
   credentials.
4. Perform native 1000×700 and reduced-motion visual review before calling the
   view accepted. Per-container Stats remains the next feature; stabilization
   and polish follow, with release still explicitly deferred.
