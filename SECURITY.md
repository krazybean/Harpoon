# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| `main` | Active development |
| `0.1.x` | Current public release line |
| Older pre-release tags | Not supported |

Users on an older `0.1.x` patch release should upgrade to the latest published `0.1.x` release before reporting a version-specific issue when practical.

## Reporting a Vulnerability

**Please do not open a public issue for security vulnerabilities.**

The preferred reporting path is **GitHub Private Vulnerability Reporting** when the repository's Security tab exposes **Report a vulnerability**. Submit the report through the private advisory flow so maintainers can reproduce, discuss, fix, and coordinate disclosure without publishing exploit details prematurely.

If private vulnerability reporting is unavailable, use only an alternative private contact method explicitly listed in the repository's GitHub Security tab. Do not send vulnerability details to an inferred or unverified email address.

Please avoid public disclosure until a fix is available and a disclosure timeline has been coordinated with the maintainers.

## What to Include

Include as much of the following as practical:

- **Description** — a clear summary of the vulnerability and affected component.
- **Steps to reproduce** — a minimal reproducer or proof of concept when available.
- **Impact** — what an attacker could achieve, such as VM escape, privilege escalation, data exposure, or denial of service.
- **Environment** — Harpoon version or commit, macOS version, Apple Silicon hardware, and relevant configuration.
- **Observed vs. expected behavior** — especially for isolation, socket permissions, networking, or lifecycle boundaries.

## Response Expectations

- We aim to **acknowledge** security reports within **72 hours**.
- We will triage the report, confirm reproducibility where possible, and assess severity and affected versions.
- Confirmed issues will be fixed and disclosed on a timeline coordinated with the reporter where practical.
- Harpoon is community-maintained, so resolution time is not a guaranteed SLA, but security reports are treated as priority work.

Reporters may be credited after disclosure if they want attribution.

## Security Scope and Boundaries

Harpoon creates and manages a Linux VM through Apple's `Virtualization.framework`, exposes a Docker-compatible Unix socket/API on the macOS host, and executes user-requested container workloads inside that VM.

Security-relevant boundaries include:

- **In scope:** VM lifecycle and isolation, Harpoon-owned host↔guest communication, Docker-compatible host socket exposure, bind-mount/path translation behavior, published-port forwarding, and privilege handling performed by Harpoon.
- **Guest runtime boundary:** Docker Engine, containerd, and BuildKit remain authoritative inside the Linux guest. Harpoon is responsible for how the macOS host connects those services to the guest.
- **Out of scope / by design:** Harpoon is not a sandbox for executing arbitrary untrusted workloads without user awareness. Running a container executes software the user requested with the resources granted by its configuration.

Reports involving VM escape, unauthorized host access through Harpoon-owned sockets or bridges, privilege escalation beyond documented behavior, unintended host-path access, or bypass of an isolation boundary are particularly relevant.

## Dependency Advisories

Harpoon monitors dependencies through GitHub security scanning, Dependabot, CodeQL, OpenSSF Scorecard, and Rust ecosystem tooling where applicable.

Some Rust advisory results may originate from transitive Tauri dependencies that are not compiled into Harpoon's macOS ARM64 production target, including Linux GTK/WebKit dependency paths. Unmaintained transitive crates may also remain until compatible upstream replacements exist.

Dependency findings are evaluated in the context of the production target rather than silently dismissed. A clean automated scan is not treated as proof that Harpoon is vulnerability-free.
