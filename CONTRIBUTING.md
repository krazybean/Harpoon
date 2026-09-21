# Contributing to Harpoon

Thanks for considering a contribution!

## Reporting Bugs

- Search existing [GitHub Issues](https://github.com/krazybean/Harpoon/issues) to avoid duplicates.
- Open a new issue with:
  - A clear title and description of expected vs. actual behavior.
  - Steps to reproduce (minimal example if possible).
  - Environment details: Harpoon version/commit, macOS version, hardware (Apple Silicon), and relevant config/logs.
- For feature requests, use an issue as well — describe the use case and why it matters.

## Proposing Changes

1. Open an issue first for non-trivial changes to discuss scope and approach.
2. Fork the repo and create a feature branch from `main`.
3. Keep changes focused and follow existing code style and conventions.
4. Add or update tests/docs where relevant.

## Building

Do not duplicate build steps here — see the canonical guide:

**[docs/building.md](docs/building.md)**

## Validation

Before opening or updating a pull request, run the repository-level validation command from the project root:

```sh
sh tools/validate.sh
```

It builds the Harpoon runtime, runs the CLI parity regression suite, verifies synchronized release versions, runs the frontend unit tests and production build, and performs a locked `cargo check` of the Tauri backend. On a clean checkout it installs frontend dependencies with `npm ci` when `node_modules` is absent.

Some VM, networking, filesystem, release-bundle, and ecosystem acceptance tests require a healthy macOS Virtualization.framework environment and remain separate from this fast repository validation path. See [docs/building.md](docs/building.md) and the milestone harnesses under `harpoon/` for those tests.

## Pull Requests

- Keep PRs small and focused — one change per PR.
- Link the related issue (e.g., `Fixes #123`).
- Explain *what* and *why*; note any trade-offs or follow-ups.
- Run `sh tools/validate.sh` and address failures before requesting review.
- Update documentation if behavior or configuration changes.
- Be responsive to review feedback.

## Security Vulnerabilities

**Do not open a public issue for security vulnerabilities.**

Please follow the private reporting process described in [SECURITY.md](SECURITY.md) (GitHub Security tab → Report a vulnerability). Do not disclose publicly until a fix and disclosure timeline have been coordinated with maintainers.
