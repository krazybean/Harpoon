import Foundation
import Virtualization
import Darwin

// MARK: - CLI handlers

func cliPrint(_ s: String) {
    print(s)
}
func cliError(_ s: String) {
    fputs(s + "\n", stderr)
}

func handleLogsPath() -> Int32 {
    cliPrint(HarpoonPaths.logFile.path)
    return 0
}

func handleDockerEnv() -> Int32 {
    cliPrint("export DOCKER_HOST=unix://\(HarpoonPaths.dockerSocketPath)")
    return 0
}

func handleVersion() -> Int32 {
    cliPrint("Harpoon 0.1.0")
    return 0
}

func printUsageFull() {
    let msg = """
    Harpoon — lightweight Docker runtime for macOS

    Usage:
      harpoon <command> [options]

    Commands:
      start       Start Harpoon in background
      stop        Stop Harpoon
      restart     Restart Harpoon
      status      Show runtime status
      logs        Show runtime logs
      config      Manage defaults
      disk        Manage persistent disk (status, resize)
      docker      Manage Docker integration
      exec        Execute command in guest (harpoon exec -- <cmd> [args...])
      shell       Open interactive shell in guest (harpoon shell)
      doctor      Diagnose common problems
      run         Run in foreground (debug)
      version     Show version
      help        Show help

    Start options:
      --cpus N                1...8 (default 2)
      --memory MiB>=512       (default 4096)
      --kernel PATH
      --initramfs PATH
      --disk PATH
      --disk-size 8G|16G|32G  First provision size (e.g. 8G, 16G) — only when no disk exists; use harpoon disk resize to grow

    Precedence: CLI flag > user config > environment > defaults
    Config: ~/Library/Application Support/Harpoon/config.json

    Disk (persistent, grow-only, sparse):
      harpoon disk status           Backing logical/physical, FS used/free, config
      harpoon disk resize 16G       Grow to 16G (VM stopped, never shrink)
      harpoon start --disk-size 16G First provision 16G (no disk), otherwise use resize

    Docker:
      harpoon docker setup   Create harpoon context (unix:///tmp/harpoon-docker.sock)
      harpoon docker status
      docker --context harpoon ps

    Examples:
      harpoon start --disk-size 16G
      harpoon docker setup
      docker --context harpoon run --rm hello-world
      docker --context harpoon compose up -d
      harpoon exec -- uname -a
      harpoon exec -- docker info
      harpoon shell

    Guest management (vsock 2377, no SSH, no TCP):
      harpoon exec -- <cmd> [args...]   Execute in guest, preserve arg boundaries
      harpoon shell                     Interactive PTY shell (/bin/sh)

    Diagnostics:
      harpoon status
      harpoon doctor
      harpoon logs --lines 100
    notes:
      harpoon start  - background managed runtime (log: ~/Library/Application Support/Harpoon/harpoon.log, socket: /tmp/harpoon-docker.sock, lock: /tmp/harpoon.lock)
      harpoon run    - foreground runtime (logs to terminal, Ctrl-C to stop)
      bare 'harpoon --cpus ...' -> alias for 'harpoon run --cpus ...' (preserves Phase 1)
    """
    fputs(msg + "\n", stderr)
}
