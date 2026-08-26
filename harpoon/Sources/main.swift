import Foundation
import Virtualization

func log(_ m: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    fputs("[\(ts)] \(m)\n", stderr)
}

func printUsage() {
    fputs("usage: harpoon [--cpus 1|2] [--memory 512|768|1024] [--kernel PATH] [--initramfs PATH] [--disk PATH]\n", stderr)
    fputs("defaults: --cpus 2 --memory 1024  (HARPOON_CPUS/HARPOON_MEMORY_MIB env fallback, CLI wins)\n", stderr)
    fputs("precedence: CLI > config > environment > defaults\n", stderr)
}

// CLI dispatch — must be before VM lifecycle side effects
let cliArgs = CommandLine.arguments
if cliArgs.count >= 2 {
    let cmd = cliArgs[1]
    // support --help for subcommands: harpoon start --help etc.
    if cliArgs.count>=3 && (cliArgs[2]=="--help" || cliArgs[2]=="-h") {
        switch cmd {
        case "start": fputs("usage: harpoon start [--cpus 1..8] [--memory 512|768|1024] [--kernel PATH] [--initramfs PATH] [--disk PATH]\n", stderr); exit(0)
        case "logs": fputs("usage: harpoon logs [--follow] [--lines N] [--path]\n", stderr); exit(0)
        case "config": fputs("usage: harpoon config <show|set|reset|path>\n", stderr); exit(0)
        case "docker": fputs("usage: harpoon docker <setup|status|remove|use|env>\n", stderr); exit(0)
        case "status": fputs("usage: harpoon status [--json]\n", stderr); exit(0)
        default: break
        }
    }
    switch cmd {
    case "start":
        exit(handleStart(args: Array(cliArgs.dropFirst(2))))
    case "stop":
        exit(handleStop())
    case "status":
        exit(handleStatus(args: Array(cliArgs.dropFirst(2))))
    case "logs":
        exit(handleLogs(args: Array(cliArgs.dropFirst(2))))
    case "restart":
        exit(handleRestart(args: Array(cliArgs.dropFirst(2))))
    case "config":
        exit(handleConfig(args: Array(cliArgs.dropFirst(2))))
    case "doctor":
        exit(handleDoctor())
    case "docker":
        exit(handleDocker(args: Array(cliArgs.dropFirst(2))))
    case "version", "--version", "-v":
        exit(handleVersion())
    case "help", "--help", "-h":
        printUsageFull()
        exit(0)
    case "run":
        break // fall through to foreground with stripped args
    default:
        if cmd.hasPrefix("-") {
            break // legacy bare flags -> foreground
        } else {
            fputs("unknown command: \(cmd)\n", stderr)
            fputs("Try 'harpoon help' for usage.\n", stderr)
            exit(2)
        }
    }
}
// also handle bare --help
if cliArgs.count==2 && (cliArgs[1]=="--help" || cliArgs[1]=="-h") {
    printUsageFull()
    exit(0)
}

// Determine foreground args
let foregroundArgs: [String]
if cliArgs.count >= 2 && cliArgs[1] == "run" {
    foregroundArgs = Array(cliArgs.dropFirst(2))
} else {
    foregroundArgs = Array(cliArgs.dropFirst(1))
}

let lifecycle = Lifecycle()
var config = RuntimeConfig.fromEnvironment()

var cliCpusProvided = false
var cliMemoryProvided = false
var cliCpusRaw: String?
var cliMemoryRaw: String?

var i = 0
while i < foregroundArgs.count {
    let a = foregroundArgs[i]
    if a == "--cpus" && i+1 < foregroundArgs.count {
        cliCpusProvided = true
        cliCpusRaw = foregroundArgs[i+1]
        if let v = Int(foregroundArgs[i+1]) { config.cpuCount = v } else { config.cpuCount = -1 }
        i += 2
    } else if a == "--cpu" && i+1 < foregroundArgs.count {
        cliCpusProvided = true
        cliCpusRaw = foregroundArgs[i+1]
        if let v = Int(foregroundArgs[i+1]) { config.cpuCount = v } else { config.cpuCount = -1 }
        i += 2
    } else if a == "--memory" && i+1 < foregroundArgs.count {
        cliMemoryProvided = true
        cliMemoryRaw = foregroundArgs[i+1]
        if let v = Int(foregroundArgs[i+1]) { config.memoryMIB = v } else { config.memoryMIB = -1 }
        i += 2
    } else if a == "--kernel" && i+1 < foregroundArgs.count {
        config.kernelURL = URL(fileURLWithPath: foregroundArgs[i+1]); i += 2
    } else if a == "--initramfs" && i+1 < foregroundArgs.count {
        config.initramfsURL = URL(fileURLWithPath: foregroundArgs[i+1]); i += 2
    } else if a == "--disk" && i+1 < foregroundArgs.count {
        config.diskURL = URL(fileURLWithPath: foregroundArgs[i+1]); i += 2
    } else if a == "--help" || a == "-h" {
        printUsage(); exit(0)
    } else {
        i += 1
    }
}

let allowed: Set<Int> = [512, 768, 1024]
if !cliMemoryProvided && !allowed.contains(config.memoryMIB) {
    log("HARPOON_MEMORY_CONFIG_WARN raw=\(config.memoryMIB) clamped to 1024 (allowed 512/768/1024) env fallback")
    config.memoryMIB = 1024
}
if !cliCpusProvided && (config.cpuCount < 1 || config.cpuCount > 8) {
    if ProcessInfo.processInfo.environment["HARPOON_CPUS"] != nil {
        log("HARPOON_CPU_CONFIG_WARN raw=\(config.cpuCount) clamped to 2 (allowed 1...8) env fallback")
        config.cpuCount = 2
    }
}

log("host \(ProcessInfo.processInfo.operatingSystemVersionString) isSupported=\(VZVirtualMachine.isSupported)")
guard VZVirtualMachine.isSupported else {
    lifecycle.fail("Virtualization not supported")
    fputs("Virtualization not supported on this host\n", stderr)
    exit(3)
}

let lockPath = "/tmp/harpoon.lock"
let harpoonLockFd: Int32 = open(lockPath, O_CREAT | O_RDWR, 0o600)
if harpoonLockFd < 0 {
    log("HARPOON_LOCK_OPEN_FAILED \(lockPath) \(String(cString:strerror(errno)))")
    fputs("HARPOON_ALREADY_RUNNING\n", stderr)
    exit(10)
}
if flock(harpoonLockFd, LOCK_EX | LOCK_NB) != 0 {
    if errno == EWOULDBLOCK {
        log("HARPOON_ALREADY_RUNNING lock \(lockPath) in use")
        fputs("HARPOON_ALREADY_RUNNING\n", stderr)
        close(harpoonLockFd)
        exit(10)
    } else {
        log("HARPOON_LOCK_FAILED \(lockPath) \(String(cString:strerror(errno)))")
        fputs("HARPOON_ALREADY_RUNNING\n", stderr)
        close(harpoonLockFd)
        exit(10)
    }
}
log("HARPOON_LOCK_ACQUIRED \(lockPath) fd=\(harpoonLockFd)")

if let err = config.validate() {
    lifecycle.fail(err)
    fputs("HARPOON_STATE \(LifecycleState.stopped.rawValue) -> \(LifecycleState.failed.rawValue) reason=\(err)\n", stderr)
    fputs("config validation failed: \(err)\n", stderr)
    exit(5)
}
log("HARPOON_CPU_CONFIG_COUNT \(config.cpuCount)")
log("HARPOON_MEMORY_CONFIG_MIB \(config.memoryMIB)")
log("HARPOON_MEMORY_CONFIG_BYTES \(config.memorySizeBytes)")
log("HARPOON_DISK_IMAGE \(config.diskURL.path)")
log("HARPOON_DISK_LOGICAL_BYTES \(config.diskLogicalBytes)")
log("HARPOON_RESOURCE_CONFIG cpus=\(config.cpuCount) memoryMiB=\(config.memoryMIB) disk=\(config.diskURL.path) diskLogicalBytes=\(config.diskLogicalBytes)")

let manager = VMManager(config: config, lifecycle: lifecycle)

lifecycle.transition(to: .starting)
let vm: VZVirtualMachine
do {
    vm = try manager.buildVM()
} catch {
    let e = error as NSError
    let reason = "invalid VZ configuration \(e.domain) \(e.code) \(e.localizedDescription)"
    lifecycle.fail(reason)
    fputs("validate failed: \(e)\n", stderr)
    exit(5)
}

lifecycle.transition(to: .booting)
log("starting VM kernel=\(config.kernelURL.lastPathComponent) initramfs=\(config.initramfsURL.lastPathComponent)")

signal(SIGPIPE, SIG_IGN)
signal(SIGINT, SIG_IGN)
signal(SIGTERM, SIG_IGN)
var shuttingDown = false
func initiateShutdown(reason: String) {
    if shuttingDown { return }
    shuttingDown = true
    lifecycle.transition(to: .stopping)
    log("SHUTDOWN_INITIATED reason=\(reason) HARPOON_SIGNAL_RECEIVED \(reason)")
    manager.serialPoll?.cancel()
    manager.stopBridges()
    if let vm = manager.vm {
        if vm.canStop {
            vm.stop { err in
                if let err = err { log("stop error: \(err)") }
                manager.cleanupEphemeral()
                lifecycle.transition(to: .stopped)
                log("SHUTDOWN_OK")
                exit(0)
            }
            DispatchQueue.main.asyncAfter(deadline: .now()+8) {
                log("SHUTDOWN_TIMEOUT force exit")
                manager.cleanupEphemeral()
                exit(0)
            }
        } else {
            manager.cleanupEphemeral()
            lifecycle.transition(to: .stopped)
            exit(0)
        }
    } else {
        manager.cleanupEphemeral()
        lifecycle.transition(to: .stopped)
        exit(0)
    }
}

let sigIntSrc = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
let sigTermSrc = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
sigIntSrc.setEventHandler { initiateShutdown(reason: "SIGINT") }
sigTermSrc.setEventHandler { initiateShutdown(reason: "SIGTERM") }
sigIntSrc.resume(); sigTermSrc.resume()
log("HARPOON_SIGNAL_HANDLERS_INSTALLED DispatchSource SIGINT/SIGTERM POSIX SIG_IGN")

let readyPoll = DispatchSource.makeTimerSource(queue: .main)
readyPoll.schedule(deadline: .now()+1, repeating: 1)
var readyPollCancelled = false
readyPoll.setEventHandler {
    _ = manager.checkSerial()
    if manager.bootReady && !readyPollCancelled {
        readyPollCancelled = true
        readyPoll.cancel()
        lifecycle.transition(to: .dockerReady)
        log("HARPOON_DOCKER_READY observed, starting bridges")
        manager.startBridges()
        lifecycle.transition(to: .running)
        log("VM Running, vsock bridge active, socket \(config.dockerSocketPath)")
        log("HARPOON_RUNNING")
        let exists = FileManager.default.fileExists(atPath: config.dockerSocketPath)
        log("HARPOON_DOCKER_SOCK_EXISTS \(exists) perms=\( (try? FileManager.default.attributesOfItem(atPath: config.dockerSocketPath)[.posixPermissions] ) ?? 0 )")
    }
    if let r = manager.bootFailedReason, !readyPollCancelled {
        readyPollCancelled = true
        readyPoll.cancel()
        lifecycle.fail("guest HARPOON_DOCKER_FAILED \(r)")
        log("GUEST_INIT_FAILURE HARPOON_DOCKER_FAILED \(r)")
        initiateShutdown(reason: "DOCKER_FAILED")
    }
}
readyPoll.resume()
manager.serialPoll = readyPoll

// M7 stop-file fallback (sandbox-safe): harpoon stop creates /tmp/harpoon-stop
let stopFile = "/tmp/harpoon-stop"
try? FileManager.default.removeItem(atPath: stopFile)
let stopPoll = DispatchSource.makeTimerSource(queue: .main)
stopPoll.schedule(deadline: .now()+1, repeating: 1)
stopPoll.setEventHandler {
    if FileManager.default.fileExists(atPath: stopFile) {
        log("HARPOON_STOP_FILE_DETECTED \(stopFile)")
        try? FileManager.default.removeItem(atPath: stopFile)
        initiateShutdown(reason: "STOP_FILE")
    }
}
stopPoll.resume()

DispatchQueue.main.asyncAfter(deadline: .now()+config.bootTimeout) {
    if !manager.bootReady && manager.bootFailedReason == nil && !readyPollCancelled {
        readyPollCancelled = true
        readyPoll.cancel()
        let tail = manager.checkSerial().suffix(800)
        let reason = "guest readiness timeout after \(config.bootTimeout)s"
        lifecycle.fail(reason)
        log("GUEST_INIT_FAILURE no HARPOON_DOCKER_READY after \(config.bootTimeout)s tail=\(tail)")
        initiateShutdown(reason: "BOOT_TIMEOUT")
        DispatchQueue.main.asyncAfter(deadline: .now()+3) { exit(6) }
    }
}

vm.start { result in
    if case .failure(let e) = result {
        let ne = e as NSError
        let reason = "VM start failure \(ne.domain) \(ne.code) \(ne.localizedDescription)"
        lifecycle.fail(reason)
        fputs("start FAIL \(ne.domain) \(ne.code) \(ne.userInfo)\n", stderr)
        log("HOST_VZ_START_FAILURE")
        manager.cleanupEphemeral()
        exit(7)
    }
    log("VM start SUCCESS state=\(vm.state.rawValue)")
}

RunLoop.main.run()
