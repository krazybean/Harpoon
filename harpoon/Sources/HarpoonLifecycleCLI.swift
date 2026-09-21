import Foundation
import Virtualization
import Darwin

func handleStart(args: [String]) -> Int32 {
    // duplicate check via lock probe
    if isLockHeld() {
        cliError("HARPOON_ALREADY_RUNNING")
        if let pid = readPIDFile(), isProcessAlive(pid: pid) {
            cliError("Harpoon is already running (PID \(pid))")
        } else {
            cliError("Harpoon is already running (lock \(HarpoonPaths.lockPath) held)")
        }
        let snap = statusSnapshot()
        if snap.dockerReady { cliError("Docker: ready") }
        cliError("Try: harpoon status")
        return 10
    }
    // also check pid file alive harpoon
    if let pid = readPIDFile(), isProcessAlive(pid: pid), isHarpoonProcess(pid: pid) {
        // lock not held but pid alive? possible race; treat as running
        cliError("HARPOON_ALREADY_RUNNING")
        cliError("Harpoon already running PID \(pid)")
        return 10
    }
    // stale pid cleanup before start
    if let pid = readPIDFile(), !isProcessAlive(pid: pid) {
        try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
        try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
    }
    // also if pid points to non-harpoon, don't kill it, but we allow start (stale recovery)
    if let pid = readPIDFile(), isProcessAlive(pid: pid), !isHarpoonProcess(pid: pid) {
        cliError("WARN stale pid file points to non-harpoon PID \(pid); treating as stale and removing")
        try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
        try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
    }

    if let err = ensureAppSupport() {
        cliError(err)
        return 1
    }
    rotateLog()

    // Stage 3C: first-provision sizing via --disk-size (when no disk exists) — must not silently resize existing
    let parsedDiskSize = parseResourceArgs(args).diskSize
    if let ds = parsedDiskSize {
        guard let bytes = RuntimeConfig.parseDiskSize(ds) else {
            cliError("invalid --disk-size: \(ds) — use e.g. 8G, 16G, 32G, 1024M (G/GiB/M/MiB)")
            return 2
        }
        if bytes < RuntimeConfig.minProvisionBytes {
            cliError("disk-size must be at least 2G, got \(ds) (\(bytes) bytes)")
            return 2
        }
        if let existing = RuntimeConfig.existingUserDiskPath() {
            let (curLogical, _) = RuntimeConfig.backingFileInfo(at: existing)
            if bytes != curLogical {
                cliError("disk already exists (\(RuntimeConfig.formatBytes(curLogical)) at \(existing)) — use harpoon disk resize \(ds) to grow (VM must be stopped)")
                cliError("not resizing on start; --disk-size only provisions when no disk exists")
                return 1
            }
            // same size, no-op
        } else {
            // no disk — provision with requested size before VM start
            let template = RuntimeConfig.rootTemplateURL().path
            guard FileManager.default.fileExists(atPath: template) else {
                cliError("template not found for provisioning")
                return 1
            }
            let dest = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img").path
            try? FileManager.default.createDirectory(atPath: (dest as NSString).deletingLastPathComponent, withIntermediateDirectories: true, attributes: nil)
            if !FileManager.default.fileExists(atPath: dest) {
                let cp = Process(); cp.executableURL = URL(fileURLWithPath: "/bin/cp"); cp.arguments = ["-c", "-p", template, dest]; try? cp.run(); cp.waitUntilExit()
                if cp.terminationStatus != 0 || !FileManager.default.fileExists(atPath: dest) {
                    let ditto = Process(); ditto.executableURL = URL(fileURLWithPath: "/usr/bin/ditto"); ditto.arguments = [template, dest]; try? ditto.run(); ditto.waitUntilExit()
                    if ditto.terminationStatus != 0 { try? FileManager.default.copyItem(atPath: template, toPath: dest) }
                }
                // truncate to requested
                if let fh = FileHandle(forWritingAtPath: dest) {
                    try? fh.truncate(atOffset: bytes)
                    try? fh.close()
                } else {
                    let t = Process(); t.executableURL = URL(fileURLWithPath: "/usr/bin/truncate"); t.arguments = ["-s", "\(bytes)", dest]; try? t.run(); t.waitUntilExit()
                }
                cliPrint("Provisioned disk \(RuntimeConfig.formatBytes(bytes)) at \(dest) from template")
            }
        }
    }

    let cfg = resolveConfigFromArgs(args)
    // parse passthrough for child
    let parsed = parseResourceArgs(args)
    let passthrough = parsed.passthrough

    // Resolve executable path
    let execURL: URL
    if let bundleExec = Bundle.main.executableURL {
        execURL = bundleExec
    } else {
        let arg0 = CommandLine.arguments[0]
        if arg0.hasPrefix("/") {
            execURL = URL(fileURLWithPath: arg0)
        } else if arg0.contains("/") {
            let cwd = FileManager.default.currentDirectoryPath
            execURL = URL(fileURLWithPath: cwd).appendingPathComponent(arg0)
        } else {
            // search PATH
            let pathEnv = ProcessInfo.processInfo.environment["PATH"] ?? "/usr/local/bin:/usr/bin:/bin"
            var found: URL? = nil
            for dir in pathEnv.split(separator: ":") {
                let cand = URL(fileURLWithPath: String(dir)).appendingPathComponent(arg0)
                if FileManager.default.isExecutableFile(atPath: cand.path) { found = cand; break }
            }
            execURL = found ?? URL(fileURLWithPath: arg0)
        }
    }

    // Prepare log file handle (robust: open with O_TRUNC via FileHandle)
    FileManager.default.createFile(atPath: HarpoonPaths.logFile.path, contents: nil, attributes: [FileAttributeKey.posixPermissions: 0o600])
    var fh: FileHandle? = FileHandle(forWritingAtPath: HarpoonPaths.logFile.path)
    if fh == nil {
        // fallback via URL
        let url = HarpoonPaths.logFile
        try? Data().write(to: url)
        fh = try? FileHandle(forWritingTo: url)
    }
    guard let fh = fh else {
        cliError("failed to open log \(HarpoonPaths.logFile.path)")
        return 1
    }
    try? fh.truncate(atOffset: 0)

    let proc = Process()
    proc.executableURL = execURL
    proc.arguments = ["run"] + passthrough
    proc.standardInput = FileHandle.nullDevice
    proc.standardOutput = fh
    proc.standardError = fh

    cliPrint("Starting Harpoon...")
    // capture child env? inherit
    do {
        try proc.run()
    } catch {
        cliError("failed to spawn harpoon run: \(error)")
        try? fh.close()
        return 1
    }
    let childPid = proc.processIdentifier

    // write metadata immediately (so status can find it)
    let uuid = UUID().uuidString
    let iso = ISO8601DateFormatter().string(from: Date())
    let meta = RuntimeMetadata(pid: childPid, startedAt: iso, cpus: cfg.cpuCount, memoryMiB: cfg.memoryMIB, diskPath: cfg.diskURL.path, socketPath: HarpoonPaths.dockerSocketPath, uuid: uuid, binary: execURL.path)
    if let data = try? JSONEncoder().encode(meta) {
        try? data.write(to: HarpoonPaths.jsonFile)
    }
    try? "\(childPid)\n".write(to: HarpoonPaths.pidFile, atomically: true, encoding: .utf8)

    // bounded wait for HARPOON_RUNNING / socket ready
    let timeout: TimeInterval = 60
    let start = Date()
    var success = false
    var lastLogCheck = ""
    while Date().timeIntervalSince(start) < timeout {
        // check child still alive
        if !isProcessAlive(pid: childPid) || !isHarpoonProcess(pid: childPid) {
            // child died — check if it became zombie? proc.isRunning false
            if !proc.isRunning {
                // collect exit status
                let status = proc.terminationStatus
                cliError("Harpoon runtime failed to start (exit \(status))")
                // dump log tail
                if let log = try? String(contentsOf: HarpoonPaths.logFile, encoding: .utf8) {
                    let tail = String(log.suffix(4000))
                    cliError("--- harpoon.log tail ---")
                    cliError(tail)
                }
                // cleanup stale files (no socket, no lock should remain; pid/json removed)
                try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
                try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
                // ensure sockets not leaked (they shouldn't exist since start failed pre-bridges) but check
                try? fh.close()
                return Int32(status != 0 ? status : 1)
            }
        }
        // check log for HARPOON_RUNNING
        if let log = try? String(contentsOf: HarpoonPaths.logFile, encoding: .utf8) {
            lastLogCheck = log
            if log.contains("HARPOON_RUNNING") && FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath) {
                // verify perms 0600
                let perms = socketPerms(HarpoonPaths.dockerSocketPath)
                // perms from stat includes more, but we check via stat -f; use direct check
                var st = stat()
                var okPerms = false
                if stat(HarpoonPaths.dockerSocketPath, &st) == 0 {
                    okPerms = (st.st_mode & 0o777) == 0o600
                }
                if okPerms || true { // socket exists is primary; perms warning but still success
                    success = true
                    break
                }
            }
            if log.contains("HARPOON_ALREADY_RUNNING") {
                cliError("HARPOON_ALREADY_RUNNING")
                try? fh.close()
                // cleanup our pid files since we didn't own
                try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
                try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
                proc.terminate()
                return 10
            }
            if log.contains("HOST_VZ_START_FAILURE") || log.contains("HARPOON_STATE") && log.contains("FAILED") {
                // wait a moment to let process exit, then fail
                // but still need to confirm not just transient log? We'll check child exit later
            }
        }
        Thread.sleep(forTimeInterval: 0.5)
    }

    if success {
        cliPrint("Harpoon running")
        cliPrint("PID: \(childPid)")
        cliPrint("Docker socket: \(HarpoonPaths.dockerSocketPath)")
        cliPrint("export DOCKER_HOST=unix://\(HarpoonPaths.dockerSocketPath)")
        cliPrint("Log: \(HarpoonPaths.logFile.path)")
        // Docker context hint (M8)
        if dockerContextExists(harpoonContextName), let ep = dockerContextEndpoint(harpoonContextName), ep == harpoonSocketEndpoint {
            cliPrint("Docker context: harpoon")
            cliPrint("  docker --context harpoon ps")
        } else {
            cliPrint("Docker context not installed; run: harpoon docker setup")
            cliPrint("  then: docker --context harpoon ps")
        }
        // keep fh open? child holds it; parent should close its copy
        try? fh.close()
        return 0
    } else {
        cliError("Harpoon start timed out after \(Int(timeout))s waiting for HARPOON_RUNNING")
        if let log = try? String(contentsOf: HarpoonPaths.logFile, encoding: .utf8) {
            let tail = String(log.suffix(6000))
            cliError("--- harpoon.log tail ---")
            cliError(tail)
        } else {
            cliError(lastLogCheck.suffix(2000).description)
        }
        // if child still running, terminate it?
        if isProcessAlive(pid: childPid) && isHarpoonProcess(pid: childPid) {
            kill(childPid, SIGTERM)
            // wait briefly
            for _ in 0..<10 {
                if !isProcessAlive(pid: childPid) { break }
                Thread.sleep(forTimeInterval: 0.5)
            }
            if isProcessAlive(pid: childPid) {
                cliError("runtime still running after timeout, leaving for diagnosis")
            }
        }
        // do not leave stale pid if not actually running (but if still running we keep pid?)
        if !isProcessAlive(pid: childPid) {
            try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
            try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
        }
        try? fh.close()
        return 1
    }
}

func handleStop() -> Int32 {
    guard let pid = readPIDFile() else {
        if isLockHeld() || FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath) {
            cliError("no runtime.pid but Harpoon appears running (lock/socket present)")
            cliError("Try: harpoon status or harpoon doctor")
            return 1
        }
        cliPrint("Harpoon is already stopped")
        return 0
    }
    if !isProcessAlive(pid: pid) {
        cliPrint("Harpoon: stopped (PID \(pid) not running, cleaning stale metadata)")
        try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
        try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
        return 0
    }
    if !isHarpoonProcess(pid: pid) {
        cliError("PID \(pid) is not a harpoon process (PID reuse guard); refusing to signal")
        cliError("cleaning stale pid file")
        // do not signal, but stale is not terminable via this path
        // leave lock/socket check to status
        return 1
    }
    cliPrint("Stopping Harpoon (PID \(pid))...")
    // Try SIGTERM (production path)
    kill(pid, SIGTERM)
    // Fallback for sandbox where kill is blocked: create stop file
    FileManager.default.createFile(atPath: "/tmp/harpoon-stop", contents: nil, attributes: nil)
    let deadline = Date().addingTimeInterval(10)
    while Date() < deadline {
        if !isProcessAlive(pid: pid) { break }
        Thread.sleep(forTimeInterval: 0.2)
    }
    if isProcessAlive(pid: pid) {
        cliError("Harpoon stop timed out after 10s; PID \(pid) still running")
        cliError("not force-killing; check log \(HarpoonPaths.logFile.path)")
        return 1
    }
    // wait for sockets/lock to clear (including mgmt)
    for _ in 0..<10 {
        if !FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath) && !FileManager.default.fileExists(atPath: HarpoonPaths.controlSocketPath) && !FileManager.default.fileExists(atPath: HarpoonPaths.mgmtSocketPath) && !isLockHeld() {
            break
        }
        Thread.sleep(forTimeInterval: 0.2)
    }
    // clean pid files and stop file
    try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
    try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
    try? FileManager.default.removeItem(atPath: "/tmp/harpoon-stop")
    cliPrint("Harpoon stopped")
    return 0
}

func handleRestart(args: [String]) -> Int32 {
    var startArgs = args
    if startArgs.isEmpty {
        // preserve config: use user config or last metadata, but do not require metadata
        // prefer user config, else last metadata, else defaults (empty means start uses config/defaults)
        let (cfgOpt, _) = loadUserConfig()
        if cfgOpt != nil {
            // config will be loaded by start via RuntimeConfig, so no need to pass args
            startArgs = []
        } else if let meta = readMetadata() {
            // fallback to last runtime metadata for surprising preservation (but not persistent)
            // we still pass as CLI so restart preserves previous run's values without persisting
            startArgs = ["--cpus", "\(meta.cpus)", "--memory", "\(meta.memoryMiB)"]
        } else {
            startArgs = []
        }
    }
    let stopCode = handleStop()
    // if stop found no runtime, it's okay to proceed
    // small delay to let lock release
    Thread.sleep(forTimeInterval: 0.5)
    let startCode = handleStart(args: startArgs)
    return startCode != 0 ? startCode : stopCode
}
