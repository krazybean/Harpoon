import Foundation

// MARK: - Docker/Podman-style container CLI parity

/// Top-level commands that Harpoon forwards to the Docker CLI while pinning the
/// engine endpoint to Harpoon's Docker-compatible socket. This is intentionally
/// a thin compatibility layer: Docker Engine remains authoritative for images,
/// containers, networks, volumes, builds, and registry operations.
let harpoonContainerCommands: Set<String> = [
    "attach", "build", "builder", "commit", "compose", "container", "cp", "create",
    "diff", "events", "export", "history", "image", "images", "import", "info",
    "inspect", "kill", "load", "login", "logout", "network", "pause", "port", "ps",
    "pull", "push", "rename", "rm", "rmi", "save", "search", "stats", "tag", "top",
    "unpause", "update", "volume", "wait", "system"
]

/// Run the installed Docker CLI with inherited stdio so interactive commands
/// (`login`, `run -it`, `exec -it`, etc.) behave naturally. We address the
/// Harpoon socket directly rather than requiring a pre-created Docker context.
func runHarpoonContainerCommand(_ command: String, args: [String]) -> Int32 {
    guard let docker = findDocker() else {
        cliError("Docker CLI not found. Harpoon container commands currently use the Docker CLI as a thin client.")
        cliError("Install the Docker CLI only; Docker Desktop is not required.")
        return 127
    }

    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: docker)
    proc.arguments = ["--host", harpoonSocketEndpoint, command] + args
    proc.standardInput = FileHandle.standardInput
    proc.standardOutput = FileHandle.standardOutput
    proc.standardError = FileHandle.standardError

    do {
        try proc.run()
        proc.waitUntilExit()
        return proc.terminationStatus
    } catch {
        cliError("failed to launch Docker CLI: \(error)")
        return 1
    }
}

private let machineRunOptions: Set<String> = [
    "--cpus", "--cpu", "--memory", "--kernel", "--initramfs", "--disk", "--help", "-h"
]

/// Backward compatibility for the old foreground runtime entry point.
/// `harpoon run` and `harpoon run --cpus ...` remain machine operations, while
/// `harpoon run IMAGE ...` gets the Docker/Podman meaning.
func shouldUseLegacyMachineRun(_ args: [String]) -> Bool {
    guard let first = args.first else { return true }
    return machineRunOptions.contains(first)
}

/// Preserve the historical runtime-log command when no container is named.
/// A positional argument switches to container logs, e.g. `harpoon logs api`.
func shouldUseLegacyMachineLogs(_ args: [String]) -> Bool {
    if args.isEmpty { return true }
    var i = 0
    while i < args.count {
        let arg = args[i]
        switch arg {
        case "--path", "--follow", "-f", "--help", "-h":
            i += 1
        case "--lines", "-n":
            guard i + 1 < args.count else { return true }
            i += 2
        default:
            if arg.hasPrefix("--lines=") { i += 1; continue }
            // Any other token is either a container name/id or a Docker logs
            // option not understood by the legacy Harpoon runtime logger.
            return false
        }
    }
    return true
}

/// The pre-parity `harpoon exec -- command ...` form executes inside the Harpoon
/// guest management service. Keep that exact spelling as a compatibility alias;
/// normal `harpoon exec CONTAINER COMMAND...` is container exec.
func shouldUseLegacyGuestExec(_ args: [String]) -> Bool {
    return args.first == "--"
}

private func runSelfAsLegacyMachineRun(_ args: [String]) -> Int32 {
    guard let executable = Bundle.main.executableURL else {
        cliError("cannot resolve Harpoon executable")
        return 1
    }
    let proc = Process()
    proc.executableURL = executable
    proc.arguments = ["run"] + args
    proc.standardInput = FileHandle.standardInput
    proc.standardOutput = FileHandle.standardOutput
    proc.standardError = FileHandle.standardError
    do {
        try proc.run()
        proc.waitUntilExit()
        return proc.terminationStatus
    } catch {
        cliError("failed to launch Harpoon foreground runtime: \(error)")
        return 1
    }
}

func printMachineUsage() {
    cliPrint("usage: harpoon machine <start|stop|restart|status|logs|run|exec|shell|disk|doctor|config> [args]")
    cliPrint("")
    cliPrint("Machine lifecycle:")
    cliPrint("  start [options]       Start the Harpoon VM in the background")
    cliPrint("  stop                  Stop the Harpoon VM")
    cliPrint("  restart [options]     Restart the Harpoon VM")
    cliPrint("  status [--json]       Show Harpoon VM/runtime status")
    cliPrint("  logs [options]        Show Harpoon runtime logs")
    cliPrint("  run [options]         Run the Harpoon VM in the foreground")
    cliPrint("  exec -- CMD...        Execute a command in the Harpoon guest")
    cliPrint("  shell                 Open a shell in the Harpoon guest")
    cliPrint("  disk <status|resize>  Inspect or grow the Harpoon disk")
    cliPrint("  doctor                Run Harpoon diagnostics")
    cliPrint("  config ...            Manage Harpoon runtime configuration")
}

func handleMachine(args: [String]) -> Int32 {
    guard let subcommand = args.first else {
        printMachineUsage()
        return 0
    }
    let rest = Array(args.dropFirst())

    switch subcommand {
    case "help", "--help", "-h":
        printMachineUsage()
        return 0
    case "start":
        return handleStart(args: rest)
    case "stop":
        if !rest.isEmpty { cliError("usage: harpoon machine stop"); return 2 }
        return handleStop()
    case "restart":
        return handleRestart(args: rest)
    case "status":
        return handleStatus(args: rest)
    case "logs":
        return handleLogs(args: rest)
    case "run":
        return runSelfAsLegacyMachineRun(rest)
    case "exec":
        return handleExec(args: rest)
    case "shell":
        return handleShell(args: rest)
    case "disk":
        return handleDisk(args: rest)
    case "doctor":
        if !rest.isEmpty { cliError("usage: harpoon machine doctor"); return 2 }
        return handleDoctor()
    case "config":
        return handleConfig(args: rest)
    default:
        cliError("unknown machine subcommand: \(subcommand)")
        printMachineUsage()
        return 2
    }
}

func printContainerParityHelp() {
    cliPrint("")
    cliPrint("Container commands (Docker/Podman-style, targeting Harpoon):")
    cliPrint("  run, ps, create, start, stop, restart, rm, kill, logs, inspect, exec")
    cliPrint("  images, pull, push, build, rmi, tag, save, load, search, history")
    cliPrint("  stats, top, port, cp, wait, pause, unpause, rename, update")
    cliPrint("  login, logout, volume, network, system, compose, info, events")
    cliPrint("")
    cliPrint("Harpoon VM lifecycle now lives under: harpoon machine ...")
    cliPrint("Backward compatibility: bare start/stop/restart/run and runtime-only logs remain machine aliases.")
}
