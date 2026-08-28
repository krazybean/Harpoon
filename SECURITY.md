# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` | Active development |

`v0.1` has not yet been published. This policy will be updated when `v0.1` is released to define its supported status.

Older pre-release tags are not supported. As the project approaches a stable release, a versioned support matrix will be published here.

## Reporting a Vulnerability

**Please do not open a public issue for security vulnerabilities.**

The preferred way to report a vulnerability is via **GitHub Private Vulnerability Reporting**:

1. Go to the repository on GitHub → **Security** tab → **Report a vulnerability** (`/security/advisories/new`).
2. Describe the issue privately — only maintainers will see it.
3. Submit the report. You will receive acknowledgement and can discuss fixes and disclosure timing privately within the advisory.

> **Note for maintainers:** GitHub Private Vulnerability Reporting must be enabled manually in repository settings (**Settings → Code security and analysis → Private vulnerability reporting**) if it is not already enabled. Until enabled, the Security tab workflow above will not be available.

If private vulnerability reporting is unavailable, please use an alternative private channel provided in the repository's GitHub Security tab rather than public disclosure. Do not invent or use an unverified email address — use only contact methods explicitly listed on GitHub.

We ask that you **do not disclose the vulnerability publicly** until a fix has been made available and a coordinated disclosure timeline has been agreed with the maintainers.

## What to Include in a Report

To help us triage quickly, please include where possible:

- **Description** — clear summary of the vulnerability and affected component.
- **Steps to reproduce** — minimal steps, proof-of-concept, or exploit code if available.
- **Impact** — what an attacker could achieve (e.g., VM escape, privilege escalation, data exposure, DoS).
- **Environment** — Harpoon version/commit, macOS version, hardware (Apple Silicon), and relevant configuration.

## Response Expectations

- We aim to **acknowledge** reports within **72 hours**.
- We will **triage** the report, confirm reproducibility, and assess severity.
- If confirmed, we will work on a fix and coordinate disclosure and release timing with you. We will keep you informed of progress within the private advisory.
- As a community-maintained project, we cannot guarantee fixed SLAs for resolution, but security reports are treated as priority.

We appreciate responsible disclosure and will credit reporters if desired once an advisory is published.

## Security Scope and Boundaries

Harpoon **creates and manages a Linux VM via Apple's Virtualization.framework**, **exposes a Docker-compatible socket/API** on the host, and **executes user-requested container workloads** inside that VM.

Security-relevant boundaries include:

- **In scope:** VM lifecycle and isolation, the Docker-compatible API/socket surface, container runtime boundaries, host↔VM communication, and handling of privileges required to manage the VM and workloads.
- **Out of scope / by design:** Harpoon is **not a sandbox for executing untrusted code without user awareness**. Running a container inherently executes the code the user requested. Users should only run workloads they trust, as containerized workloads can access resources granted by their configuration.

Reports concerning VM escape, unauthorized host access via the exposed API, privilege escalation beyond intended design, or bypass of documented isolation boundaries are of particular interest.

### Rust dependency advisories

Harpoon monitors Rust dependencies using RustSec and GitHub security
scanning.

The current dependency graph may report informational RustSec advisories
originating from transitive Tauri dependencies. These currently consist
of unmaintained-crate warnings and a `glib` unsoundness advisory.

The GTK3-related advisories are associated with Tauri's Linux WebKit/GTK
dependency graph and are not compiled into Harpoon's macOS ARM64
production target.

The remaining `unic` advisories identify unmaintained transitive crates
used by Tauri; no patched versions are currently available.

`cargo audit` currently reports no known vulnerabilities. These
transitive advisories will be monitored and updated as compatible
upstream Tauri dependencies become available.

## Reporting a Vulnerability

Please do not report security vulnerabilities through public GitHub issues.

Report suspected vulnerabilities privately using Harpoon's GitHub
Security Advisories:

https://github.com/krazybean/Harpoon/security/advisories/new

Please include enough information to reproduce and assess the issue,
including the affected Harpoon version, environment, observed behavior,
expected behavior, and reproduction steps when available.

Security reports will be acknowledged as soon as practical. Please allow
reasonable time for investigation and remediation before public
disclosure.