<<<<<<< HEAD
Hello World
=======
# Harpoon

> Cause damn the whales

Harpoon is a lightweight, Docker-compatible container runtime environment for macOS,
designed around one primary goal:

> Run ordinary Docker development workloads on macOS without allowing the underlying
> Linux virtual machine to permanently consume an excessive portion of host memory.

Harpoon is not a new container engine. It provides the macOS virtualization substrate
required to run an existing Linux container stack while preserving compatibility with
the Docker ecosystem.

## Product Thesis

Linux containers cannot run directly on the macOS kernel. Existing macOS container
products therefore run a Linux virtual machine underneath Docker or another
OCI-compatible runtime.

Harpoon accepts that architectural requirement but aggressively minimizes its cost.
Its core design philosophy is:

> The virtual machine should consume resources only while workloads require them,
> and should return those resources to macOS as aggressively and safely as possible
> afterward.

The first-class optimization target is therefore not simply low *configured* VM
memory, but low **resident host memory during idle and post-workload states**.

## Expected Workflow

Developers keep using their existing Docker tooling:

```bash
harpoon start
harpoon docker setup

docker --context harpoon compose up -d --build
docker --context harpoon compose ps
docker --context harpoon compose logs
docker --context harpoon compose down
```

Harpoon does not invent replacement commands for Docker operations already supported
by the Docker API. Harpoon commands manage Harpoon itself:

```bash
harpoon start                # background (defaults: cpus 2, memory 1024)
harpoon stop
harpoon status               # human, --json for machines
harpoon logs [--follow] [--lines N] [--path]
harpoon config show|set|reset  # persistent defaults (~/Library/Application Support/Harpoon/config.json)
harpoon docker setup|status|use  # Docker context (unix:///tmp/harpoon-docker.sock)
harpoon doctor               # diagnostics
harpoon version              # Harpoon 0.1.0-dev
```

## Key Properties

- **Docker-compatible**: exposes a Docker API Unix socket at `~/.harpoon/docker.sock`;
  works with the Docker CLI, Compose, LazyDocker, IDE integrations, and Testcontainers.
- **Memory-first**: dynamic VM memory management with aggressive, observable
  reclamation of unused guest memory back to macOS.
- **Persistent**: stopping Harpoon never destroys images, volumes, containers, or
  Docker metadata.
- **Appliance guest**: a minimal Linux VM containing only what Docker workloads need —
  no desktop environment, no unnecessary services.
- **Boring virtualization**: built on Apple's `Virtualization.framework`, written in Rust.

## Installation

```sh
tar xzf dist/harpoon-0.1.0-dev-darwin-arm64.tar.gz
./dist/harpoon-0.1.0-dev-darwin-arm64/install.sh  # to /usr/local (needs sudo)
# or from repo
bash harpoon/install.sh
harpoon version   # 0.1.0-dev
harpoon doctor    # 11 PASS
cd /tmp && harpoon doctor  # relocatable, no repo required
```

Staged: `dist/harpoon-0.1.0-dev-darwin-arm64` (bin 802K, kernel 33M, initramfs 14M, root 2.0G/962M) + tar.gz 289M. See [Installation](docs/installation.md) and [Phase 2 Acceptance](docs/phase2-acceptance.md).

Status: Phase 2 M7-M11 PASS, M12 CONDITIONAL PASS (host VZErrorDomain 1 transient blocks live VM in this env; install/CLI/persistence proven).

## Native macOS Virtualization

Harpoon is built directly on Apple's native `Virtualization.framework`.

Rather than bundling a separate third-party hypervisor, Harpoon uses Apple's native `Virtualization.framework` for Linux virtualization, including Virtio networking, block storage, VirtioFS, vsock, and memory ballooning.

## Lightweight by Design

In feasibility testing on the same Mac, Harpoon's VM used approximately 386 MB of physical memory at idle versus approximately 954 MB for Docker Desktop. Under the same nginx + Redis + PostgreSQL workload, Harpoon used approximately 919 MB versus approximately 1.76 GB.

These figures are measurements from the development test system and are not universal performance guarantees — see [Architecture](docs/architecture.md) for methodology, configurations, and workload equivalence.

## Platform Scope (v0.1)

- macOS on Apple Silicon
- ARM64 Linux guests
- Docker Engine + Docker Compose compatibility
- Local developer workloads

## Documentation

| Document                                        | Purpose                                          |
| ----------------------------------------------- | ------------------------------------------------ |
| [Architecture](docs/architecture.md)            | Components, boundaries, and data flows           |
| [Lifecycle](docs/lifecycle.md)                | CLI, background lifecycle, and process model   |
| [Docker Integration](docs/docker-integration.md) | Docker context integration and workflows       |
| [Compose](docs/compose.md)                      | Compose workflow and fixture                     |
| [Configuration](docs/configuration.md)        | Persistent config and precedence               |
| [Troubleshooting](docs/troubleshooting.md)    | Doctor, exit codes, and common fixes           |
| [Requirements](docs/requirements.md)            | v0.1 scope, acceptance tests, and non-goals      |
| [Memory Model](docs/memory-model.md)            | Memory taxonomy, policy engine, and reclamation  |
| [MVP](docs/mvp.md)                                  | MVP scope and Must/Should/Post-MVP         |
| [Compatibility](docs/compatibility.md)            | Docker/tooling and host compatibility      |
| [Risks](docs/risks.md)                            | Risk register and mitigations              |
| [Decisions](docs/decisions/)                    | Architecture decision records (ADRs)             |

## Status

Pre-release. v0.1 targets the MVP acceptance workflows described in
[Requirements](docs/requirements.md).

## License

MIT — see [LICENSE](LICENSE).

>>>>>>> 28101b8 (saving initial changes)
