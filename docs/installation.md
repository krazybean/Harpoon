# Install Harpoon on macOS

This is the installation guide for people using a **published Harpoon.app DMG**, not building Harpoon from source. For compilation and historical standalone CLI packaging, see [Building](building.md) and [Distribution](distribution.md).

## Choose a release

| Release | Use |
|---|---|
| [Harpoon v2.0.0-rc.4 universal DMG](https://github.com/krazybean/Harpoon/releases/tag/v2.0.0-rc.4) | **Testing prerelease.** Apple Silicon only has been run in acceptance; Intel guest boot remains unverified. |
| [Harpoon v0.1.1 arm64 DMG](https://github.com/krazybean/Harpoon/releases/tag/v0.1.1) | **Latest stable** desktop release, Apple Silicon. |

macOS 12 or later is required for the rc.4 package. You do not need Docker Desktop, Rust, Xcode, Node.js or a source checkout to use the bundled app. The rc.4 **runtime** passed isolated Apple Silicon acceptance, but installation and first launch of the **shipping desktop app** have not been accepted, so treat it as experimental software.

## Install rc.4

1. Download [`Harpoon-2.0.0-rc.4-universal.dmg`](https://github.com/krazybean/Harpoon/releases/download/v2.0.0-rc.4/Harpoon-2.0.0-rc.4-universal.dmg) from the GitHub release.
2. Check `shasum -a 256 Harpoon-2.0.0-rc.4-universal.dmg` against the [published checksum](https://github.com/krazybean/Harpoon/releases/download/v2.0.0-rc.4/SHA256SUMS). Expected: `7dd7cbf2a190f192a886419b1ee84cf38c34ad9f557d0d7c9af8a55e45608d51`. If it differs, stop.
3. Open the DMG and copy **Harpoon.app** to **/Applications**.
4. Launch Harpoon. It may offer to add its bundled `harpoon` and `docker` commands to new Terminal sessions through `/etc/paths.d/harpoon`; this is optional and requires an administrator password. Declining does not prevent using the desktop.
5. Start the Harpoon runtime from the desktop and check its status. The first image pull requires internet access. Developer ID signing, notarization and stapling were verified for rc.4, so do not disable Gatekeeper to install it.

### CLI verification (optional)

After accepting the Terminal command registration, open a **new** Terminal window:

```sh
harpoon status
harpoon doctor
docker --context harpoon version
docker --context harpoon run --rm hello-world
```

The `harpoon` Docker context points at `unix:///tmp/harpoon-docker.sock` and is set up without changing your default Docker context. If necessary, `harpoon docker setup` creates or repairs it. If you have a separate Docker CLI, invoke it with `--context harpoon` to avoid talking to the wrong engine. Docker Compose v2 is separately needed only for Compose workflows.

## A local source deployment

In **Deployments**, register a Git repository or local checkout. For a Dockerfile project, Harpoon uses its Dockerfile; supported npm and Python pip/uv projects without a Dockerfile can be built using a generated recipe. Configure install/build/start settings, variables and Keychain secrets, review the target local port and health check, then explicitly confirm the deployment. The stages recorded by Harpoon distinguish build, preliminary health check, cutover and final check.

Deployments run on this Mac only. They require an attending person; no automatic deploy-on-push, remote hosting, branch preview environment or uptime guarantee is offered. The source-deployment path of the **packaged rc.4 desktop** has not been run in acceptance. [See the specific test evidence](releases/v2.0.0-rc.4.md).

## Data, upgrades and removal

**Back up both support directories before installing rc.4 over an existing Harpoon installation:**

```text
~/Library/Application Support/Harpoon
~/Library/Application Support/com.harpoon.desktop
```

The first holds runtime configuration, persistent guest disk and operational state. The second holds desktop/deployment state, including the SQLite deployment journal. Deployment secrets are stored in the macOS Keychain (not in the SQLite journal). Container images, volumes and data live in the guest disk; **removing the app bundle does not remove them automatically**.

An existing root disk is reused and is never silently replaced merely because a new app version ships. New disks start at 4 GiB logical sparse capacity by default; enlarging one is a grow-only, explicitly confirmed operation which interrupts running containers. It does not reclaim already allocated blocks; `harpoon disk reclaim --yes` requests a guest trim and does not prune Docker data. [Disk configuration and recovery](configuration.md).

**Upgrades and rollback are not verified for rc.4.** Do not infer that a successful new-install smoke test proves a stateful upgrade is safe. Before trying an upgrade or downgrade: stop workloads deliberately, retain verified backups of **both** support directories and any external application data, and be prepared to restore the previous app *and* data together if needed. Older builds can be incompatible with future deployment-journal schema changes; rc.4 reads schema 1 and 2 but does not write schema 2.

To stop using the DMG-installed app, stop the Harpoon runtime, quit the desktop, then remove **Harpoon.app** from Applications. If you accepted the optional Terminal-path integration, an administrator can remove `/etc/paths.d/harpoon` separately. This is **not** a data purge: do not delete either support directory or Keychain secrets unless you've independently verified backups and intend to remove all persistent Harpoon state. The repository's `harpoon/install.sh` and `harpoon/uninstall.sh` describe a separate source/CLI installation path, not an automatic uninstaller for the DMG desktop.

## Troubleshooting

- **Command not found:** open a new Terminal after the optional PATH registration. Otherwise use the desktop, or consult [Building](building.md) for source/CLI instructions.
- **Wrong Docker engine:** inspect `docker context ls`; use `docker --context harpoon ...`, not an assumed default. `harpoon docker setup` repairs its endpoint without selecting it as default.
- **Docker is not ready:** check `harpoon status` and `harpoon doctor`; inspect [runtime logs and diagnostics](lifecycle.md). Startup may fail with `VZErrorDomain 1` or an intermittent guest fault; these are **unresolved** and aren't a reason to delete the root disk or disable Gatekeeper.
- **Images won't pull:** check the Mac's network and the guest's Docker readiness. A running Docker context alone does not establish that a registry is reachable.
- **Low disk space:** inspect `harpoon disk status`; expansion requires confirmation and temporarily stops the VM. Reclaim and cleanup are separate operations with different effects.
- **Existing app doesn't restart after an engine restart:** older deployments may still have Docker restart policy `no`; see [application recovery](deployment-recovery.md) for the dry-run-first migration. Do not apply it to production workloads without authorization.

If you encounter a bug, use [GitHub Issues](https://github.com/krazybean/Harpoon/issues) with your Harpoon version, macOS version, Mac architecture, and diagnostic output. For vulnerabilities, follow [private reporting](../SECURITY.md).
