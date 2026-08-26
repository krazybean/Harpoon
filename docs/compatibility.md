# Compatibility

## Docker API

- Target Docker Engine API version negotiated via Docker CLI; Harpoon proxies bytes unchanged. No filtering.
- Must work with Docker CLI ≥ 24, Compose v2, LazyDocker, Testcontainers (Java/Go/Node).
- BuildKit enabled by default in Docker Engine inside guest.

## Images / Build

- Standard Dockerfiles, no Harpoon modifications.
- `docker build` via BuildKit; `docker pull` from Docker Hub, private registries (credential helpers SHOULD work via host env passthrough experiment)
- ARM64 native; x86_64 images: require Rosetta or qemu — not MVP; error message must name arch mismatch.

## Filesystem

- See architecture.md VirtioFS correctness list (watchers, symlinks, permissions, UID/GID, Git, Node, case sensitivity, concurrency).
- APFS case-insensitive host → ext4 case-sensitive container: document behavior; prefer case-sensitive test dir for code.

## Network

- `VZNATNetworkDeviceAttachment` for VM NAT; `localhost` port-forward required (Spike 3).
- IPv4 MUST; IPv6 OPEN QUESTION; VPN/proxy POST-MVP; corporate CA bundles SHOULD be mountable.

## Host Requirements

- macOS 13+ (Ventura) minimum for `VZGenericPlatformConfiguration`; test on macOS 26.5 host in Spike.
- Apple Silicon only.
- Entitlement `com.apple.security.virtualization` required; binary must be ad-hoc or Developer-ID signed with entitlement.

## Tooling Matrix

| Client | Expected | Spike |
|---|---|---|
| docker CLI | MUST | 2 |
| compose | SHOULD | 2 |
| lazydocker | SHOULD | 2 |
| Testcontainers | SHOULD | 2 |
| IDE docker integrations | best-effort | 2 |

