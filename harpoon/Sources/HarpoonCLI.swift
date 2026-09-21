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

func handleStatus(args: [String]=[]) -> Int32 {
    if args.contains("--json") {
        let snap = statusSnapshot()
        var json: [String: Any] = [
            "state": snap.state,
            "socketPath": HarpoonPaths.dockerSocketPath,
            "lockPath": HarpoonPaths.lockPath,
            "logPath": HarpoonPaths.logFile.path,
            "dockerReady": snap.dockerReady,
            "sockExists": snap.sockExists,
            "lockHeld": snap.lockHeld,
            "mgmtSocketPath": HarpoonPaths.mgmtSocketPath,
            "mgmtReady": isMgmtServiceReachable()
        ]
        if let pid = snap.pid { json["pid"] = pid }
        if let m = snap.meta {
            json["cpus"] = m.cpus
            json["memoryMiB"] = m.memoryMiB
            json["diskPath"] = m.diskPath
        } else {
            // try config for defaults
            let cfg = RuntimeConfig.fromEnvironment()
            json["cpus"] = cfg.cpuCount
            json["memoryMiB"] = cfg.memoryMIB
            json["diskPath"] = cfg.diskURL.path
        }
        if let data = try? JSONSerialization.data(withJSONObject: json, options: [.sortedKeys, .prettyPrinted]),
           let s = String(data: data, encoding: .utf8) {
            print(s)
        }
        return 0
    }
    let snap = statusSnapshot()
    // clean obviously stale pid metadata if pid dead or not harpoon, and no lock/socket
    if snap.state == "stale" {
        // optionally clean stale pid file if process gone and no lock
        if let pid = snap.pid, !snap.alive {
            // stale pid file pointing to dead process -> remove
            try? FileManager.default.removeItem(at: HarpoonPaths.pidFile)
            try? FileManager.default.removeItem(at: HarpoonPaths.jsonFile)
        } else if let pid = snap.pid, snap.alive, !snap.isHarpoon {
            // pid reused for unrelated process, do not remove? but mark stale
        }
        if snap.pid != nil && !snap.alive && !snap.lockHeld && !snap.sockExists {
            cliPrint("Harpoon: stopped (stale pid cleaned)")
            cliPrint("PID: \(snap.pid ?? 0) (stale, not running)")
            return 0
        }
    }
    switch snap.state {
    case "running":
        cliPrint("Harpoon: running")
        if let pid = snap.pid { cliPrint("PID: \(pid)") }
        cliPrint("VM: running")
        cliPrint("Docker: ready")
        let mgmtReady = isMgmtServiceReachable()
        cliPrint("Management: \(mgmtReady ? "ready" : "not ready")")
        if let m = snap.meta {
            cliPrint("CPUs: \(m.cpus)")
            cliPrint("Memory: \(m.memoryMiB) MiB")
            cliPrint("Socket: \(m.socketPath)")
            cliPrint("Disk: \(m.diskPath)")
        } else {
            // fallback to probing log? just not print
            if FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath) {
                cliPrint("Socket: \(HarpoonPaths.dockerSocketPath)")
            }
        }
        cliPrint("Lock: \(HarpoonPaths.lockPath)")
        cliPrint("Log: \(HarpoonPaths.logFile.path)")
    case "starting":
        cliPrint("Harpoon: starting")
        if let pid = snap.pid { cliPrint("PID: \(pid)") }
        cliPrint("VM: starting")
        cliPrint("Docker: not ready")
        if let m = snap.meta { cliPrint("CPUs: \(m.cpus) Memory: \(m.memoryMiB) MiB") }
    case "degraded":
        cliPrint("Harpoon: degraded")
        if let pid = snap.pid { cliPrint("PID: \(pid) alive=\(snap.alive) harpoon=\(snap.isHarpoon)") }
        cliPrint("Lock held: \(snap.lockHeld)")
        cliPrint("Socket exists: \(snap.sockExists) ready=\(snap.dockerReady)")
    case "stale":
        cliPrint("Harpoon: stale")
        if let pid = snap.pid { cliPrint("PID: \(pid) (stale)") }
        cliPrint("Lock held: \(snap.lockHeld) Socket: \(snap.sockExists)")
        if snap.pid != nil && !snap.alive {
            cliPrint("PID file points to dead process; run 'harpoon start' to recover")
        } else if snap.pid != nil && !snap.isHarpoon {
            cliPrint("PID file points to non-harpoon process (PID reuse); not signaling")
        }
    default:
        cliPrint("Harpoon: stopped")
        if let pid = snap.pid { cliPrint("PID: \(pid) stale") }
    }
    return 0
}

func handleLogs(args: [String]) -> Int32 {
    if args.contains("--path") {
        // print resolved log path (check both locations)
        let candidates = [HarpoonPaths.logFile.path, "/tmp/harpoon-runtime/harpoon.log", FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/harpoon.log").path]
        for cand in candidates {
            if FileManager.default.fileExists(atPath: cand) { cliPrint(cand); return 0 }
        }
        cliPrint(HarpoonPaths.logFile.path)
        return 0
    }
    if args.contains("--help") || args.contains("-h") {
        cliPrint("usage: harpoon logs [--follow] [--lines N] [--path]")
        return 0
    }
    var follow = false
    var lines: Int? = nil
    var i = 0
    while i < args.count {
        if args[i] == "--follow" || args[i] == "-f" { follow = true; i += 1 }
        else if args[i] == "--lines" && i+1 < args.count {
            if let v = Int(args[i+1]) { lines = v } else { cliError("invalid --lines: \(args[i+1])"); return 1 }
            i += 2
        }
        else if args[i].hasPrefix("--lines=") {
            let vStr = args[i].components(separatedBy: "=").last ?? ""
            if let v = Int(vStr) { lines = v } else { cliError("invalid --lines: \(vStr)"); return 1 }
            i += 1
        }
        else if args[i] == "-n" && i+1 < args.count {
            if let v = Int(args[i+1]) { lines = v } else { cliError("invalid -n: \(args[i+1])"); return 1 }
            i += 2
        }
        else if args[i].hasPrefix("-") {
            cliError("unknown option: \(args[i])")
            cliPrint("usage: harpoon logs [--follow] [--lines N] [--path]")
            return 1
        }
        else { i += 1 }
    }
    // resolve actual log path (check both)
    var logPath = HarpoonPaths.logFile.path
    let candidates2 = [HarpoonPaths.logFile.path, "/tmp/harpoon-runtime/harpoon.log", FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/harpoon.log").path, "/tmp/harpoon.log"]
    for cand in candidates2 {
        if FileManager.default.fileExists(atPath: cand) { logPath = cand; break }
    }
    let fm = FileManager.default
    if !fm.fileExists(atPath: logPath) {
        cliError("no log at \(logPath)")
        // also check legacy /tmp/harpoon.log
        if fm.fileExists(atPath: "/tmp/harpoon.log") {
            cliError("legacy log at /tmp/harpoon.log")
        }
        return 1
    }
    if follow {
        // simple tail -f implementation via cat + follow loop
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/tail")
        proc.arguments = ["-F", logPath]
        proc.standardInput = FileHandle.nullDevice
        // tail will run until interrupted; we forward signals
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            cliError("tail failed: \(error)")
            return 1
        }
        return 0
    } else if let n = lines {
        // tail -n
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/tail")
        proc.arguments = ["-n", "\(n)", logPath]
        let pipe = Pipe()
        proc.standardOutput = pipe
        do {
            try proc.run()
            proc.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            if let s = String(data: data, encoding: .utf8) { print(s, terminator: "") }
        } catch {
            // fallback read file
            if let s = try? String(contentsOfFile: logPath, encoding: .utf8) {
                let all = s.components(separatedBy: "\n")
                let tail = all.suffix(n).joined(separator: "\n")
                print(tail)
            }
        }
        return 0
    } else {
        if let s = try? String(contentsOfFile: logPath, encoding: .utf8) {
            print(s, terminator: "")
        } else {
            cliError("failed to read \(logPath)")
            return 1
        }
        return 0
    }
}

func handleDoctor() -> Int32 {
    var passed=0, warned=0, failed=0
    func check(_ ok: Bool, _ msg: String, warn: Bool=false) {
        if ok { cliPrint("PASS  \(msg)"); passed+=1 }
        else if warn { cliPrint("WARN  \(msg)"); warned+=1 }
        else { cliPrint("FAIL  \(msg)"); failed+=1 }
    }
    cliPrint("Harpoon Doctor")
    cliPrint("")
    // HOST
    let ver = ProcessInfo.processInfo.operatingSystemVersionString
    check(true, "macOS \(ver)")
    #if arch(arm64)
    check(true, "architecture arm64")
    #else
    check(false, "architecture not arm64", warn:true)
    #endif
    check(VZVirtualMachine.isSupported, "Virtualization.framework available")
    // HARPOON
    let bin = Bundle.main.executableURL?.path ?? CommandLine.arguments[0]
    check(FileManager.default.isExecutableFile(atPath: bin), "binary \(bin)")
    let appDir = HarpoonPaths.appSupportDir.path
    check(FileManager.default.isWritableFile(atPath: appDir) || FileManager.default.fileExists(atPath: appDir), "runtime directory writable \(appDir)")
    let runtime = RuntimeConfig.fromEnvironment()
    let kernel = runtime.kernelURL.path
    check(FileManager.default.fileExists(atPath: kernel), "kernel \(kernel)")
    let initramfs = runtime.initramfsURL.path
    check(FileManager.default.fileExists(atPath: initramfs), "initramfs \(initramfs)")
    let disk = runtime.diskURL.path
    let diskExists = FileManager.default.fileExists(atPath: disk)
    check(diskExists, "disk \(disk)")
    if diskExists, let attrs = try? FileManager.default.attributesOfItem(atPath: disk), let sz = attrs[.size] as? UInt64 {
        cliPrint("INFO  disk logical bytes \(sz)")
    }
    let snap = statusSnapshot()
    let lockHeld = isLockHeld()
    check(!lockHeld || snap.state=="running" || snap.state=="starting", "lock state (held=\(lockHeld) state=\(snap.state))", warn: lockHeld && snap.state=="stopped")
    // PID
    if let pid = snap.pid {
        check(snap.alive, "PID \(pid) alive", warn:!snap.alive)
        if snap.alive { check(snap.isHarpoon, "PID is harpoon") }
    } else {
        cliPrint("INFO  no PID file (stopped)")
    }
    // socket
    let sockExists = FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath)
    if snap.state=="running" {
        check(sockExists, "socket \(HarpoonPaths.dockerSocketPath)")
        var st = stat()
        let permsOk = stat(HarpoonPaths.dockerSocketPath, &st)==0 && (st.st_mode & 0o777)==0o600
        check(permsOk, "socket 0600")
    } else {
        cliPrint("INFO  socket exists=\(sockExists) (expected when running)")
    }
    // DOCKER — decomposed per 3B
    if let docker = findDocker() {
        check(true, "Docker CLI ................. PASS \(docker)")
        let (code, out, _) = runDocker(["--version"])
        if code==0 { cliPrint("INFO  \(out.trimmingCharacters(in: .whitespacesAndNewlines))") }
        let (compInstalled, compVer) = dockerComposeVersion()
        if compInstalled {
            check(true, "Docker Compose plugin ...... PASS \(compVer)")
        } else {
            check(false, "Docker Compose plugin ...... FAIL — not installed (checked: docker compose version) — Install Docker Compose v2 plugin", warn:true)
        }
        if dockerContextExists(harpoonContextName) {
            if let ep = dockerContextEndpoint(harpoonContextName) {
                check(ep==harpoonSocketEndpoint, "Harpoon context ............ PASS \(ep)")
                if ep != harpoonSocketEndpoint { cliPrint("INFO  expected \(harpoonSocketEndpoint) — run: harpoon docker setup") }
            }
        } else {
            check(false, "Harpoon context ............ FAIL — not installed (run harpoon docker setup) — endpoint \(harpoonSocketEndpoint)", warn:true)
        }
        if let cur = currentDockerContext() { cliPrint("INFO  active context \(cur) (default unchanged after setup)") }
        if snap.state=="running" && sockExists {
            let (c, vOut, _) = runDocker(["--context", "harpoon", "version"])
            if c==0 {
                let verLine = vOut.components(separatedBy: "\n").first?.trimmingCharacters(in: .whitespaces) ?? ""
                check(true, "Docker Engine .............. PASS \(verLine)")
            } else {
                check(false, "Docker Engine .............. FAIL — not reachable via harpoon context")
            }
        } else if snap.state=="running" {
            check(false, "Harpoon socket ............. FAIL — socket not found (expected \(HarpoonPaths.dockerSocketPath))")
        } else {
            // VM stopped: CLI/context can still PASS, socket/Engine report stopped accurately, don't mark context invalid
            cliPrint("INFO  Harpoon socket ............. not running (expected when VM stopped) exists=\(sockExists) — CLI/context checks above still valid")
            cliPrint("INFO  Docker Engine .............. not running (VM stopped) — start with harpoon start")
        }
        let creds = credsStoreStatus()
        if creds.status == "WARN" {
            check(false, "Docker credential helper ... WARN — config references docker-credential-desktop but helper not installed (credsStore: \(creds.store ?? "")) — Harpoon does not require Docker Desktop", warn:true)
            if let w = creds.warning { cliPrint("WARN  \(w)") }
        } else if let s = creds.store {
            check(true, "Docker credential helper ... PASS (credsStore: \(s))")
        } else {
            check(true, "Docker credential helper ... PASS")
        }
    } else {
        check(false, "Docker CLI ................. FAIL — Docker CLI not installed/found (checked PATH, /opt/homebrew/bin/docker, /usr/local/bin/docker, /usr/bin/docker) — Install Docker CLI; Docker Desktop NOT required", warn:false)
        check(false, "Docker Compose plugin ...... FAIL — Docker CLI not found", warn:true)
        check(false, "Harpoon context ............ FAIL — Docker CLI not found", warn:true)
        cliPrint("INFO  Harpoon socket ............. skipped — Docker CLI not found")
        cliPrint("INFO  Docker Engine .............. skipped — Docker CLI not found")
        let creds = credsStoreStatus()
        if creds.status == "WARN" {
            check(false, "Docker credential helper ... WARN — \(creds.warning ?? "")", warn:true)
        }
    }
    // RUNTIME
    if snap.state=="stale" { check(false, "stale PID metadata", warn:true) }
    if snap.state=="degraded" { check(false, "degraded state", warn:true) }
    // Stage 3A mgmt channel
    let mgmtExists = FileManager.default.fileExists(atPath: HarpoonPaths.mgmtSocketPath)
    if snap.state=="running" {
        check(mgmtExists, "mgmt socket \(HarpoonPaths.mgmtSocketPath)")
        if mgmtExists {
            var st = stat()
            let permsOk = stat(HarpoonPaths.mgmtSocketPath, &st)==0 && (st.st_mode & 0o777)==0o600
            check(permsOk, "mgmt socket 0600")
            check(isMgmtServiceReachable(), "mgmt service reachable (vsock 2377)")
        }
        if let log = try? String(contentsOfFile: HarpoonPaths.logFile.path, encoding: .utf8) {
            check(log.contains("HARPOON_MGMT_READY"), "HARPOON_MGMT_READY in log")
        } else {
            check(false, "HARPOON_MGMT_READY in log")
        }
    } else {
        cliPrint("INFO  mgmt exists=\(mgmtExists) (expected when running)")
    }
    // STORAGE (Stage 3C) — immutable template vs mutable user disk, sparse, grow-only
    let backing = diskBackingPathForStatus()
    let backingExists = FileManager.default.fileExists(atPath: backing)
    if backingExists {
        let (logical, physical) = RuntimeConfig.backingFileInfo(at: backing)
        check(true, "Persistent disk ............ PASS \(backing)")
        cliPrint("INFO  Logical capacity ........... \(RuntimeConfig.formatBytes(logical)) (\(logical) bytes)")
        cliPrint("INFO  Host allocation ............ \(RuntimeConfig.formatBytes(physical)) (\(physical) bytes) sparse")
        if logical < RuntimeConfig.minProvisionBytes {
            check(false, "Persistent disk logical < 2G (\(RuntimeConfig.formatBytes(logical))) — should be >=2G (template size, existing disks <32G remain valid)", warn:true)
        }
        // Config vs actual mismatch (Stage 3C B)
        if let cfgDiskStr = loadUserConfig().0?.diskSize, let cfgBytes = RuntimeConfig.parseDiskSize(cfgDiskStr) {
            if cfgBytes != logical {
                cliPrint("INFO  Configured disk-size ...... \(cfgDiskStr) (\(RuntimeConfig.formatBytes(cfgBytes))) vs actual \(RuntimeConfig.formatBytes(logical)) — not auto-resized; run harpoon disk resize \(cfgDiskStr) (VM stopped, grow-only)")
                if cfgBytes > logical {
                    check(false, "Configured disk-size \(cfgDiskStr) > actual \(RuntimeConfig.formatBytes(logical)) — pending explicit resize", warn:true)
                }
            }
        }
        if let info = guestFilesystemInfoViaMgmt(), let cap = info.capacity {
            cliPrint("INFO  Filesystem capacity ........ \(RuntimeConfig.formatBytes(cap))")
            if let free = info.free { cliPrint("INFO  Filesystem free ............ \(RuntimeConfig.formatBytes(free))") }
            if logical > cap {
                check(false, "Persistent disk backing \(RuntimeConfig.formatBytes(logical)) > filesystem \(RuntimeConfig.formatBytes(cap)) — pending resize (interrupted)", warn:true)
            } else {
                check(true, "Filesystem capacity matches backing")
            }
            if let free = info.free, free < 512 * 1024 * 1024 {
                check(false, "Filesystem low free space \(RuntimeConfig.formatBytes(free))", warn:true)
            }
        } else if snap.state=="running" {
            check(false, "Filesystem capacity ........ FAIL — mgmt not reachable for live stats", warn:true)
        } else {
            cliPrint("INFO  Filesystem capacity ........ unavailable (VM stopped) — backing above is authoritative")
        }
        // host free space
        if let attrs = try? FileManager.default.attributesOfFileSystem(forPath: backing), let freeHost = attrs[.systemFreeSize] as? UInt64, freeHost < 2 * 1024 * 1024 * 1024 {
            check(false, "Host filesystem low free \(RuntimeConfig.formatBytes(freeHost)) — sparse growth may fail", warn:true)
        }
    } else {
        let desired = RuntimeConfig.desiredProvisionBytes()
        check(false, "Persistent disk ............ not yet provisioned — will be \(RuntimeConfig.formatBytes(desired)) at \(backing) on first start", warn:true)
    }
    // config
    let (cfg, err) = loadUserConfig()
    if let e = err { check(false, "config invalid: \(e)") } else if cfg != nil { cliPrint("INFO  user config present") }
    cliPrint("")
    cliPrint("\(passed) passed, \(warned) warnings, \(failed) failures")
    return failed>0 ? 1 : 0
}

func handleLogsPath() -> Int32 {
    cliPrint(HarpoonPaths.logFile.path)
    return 0
}

func handleDockerEnv() -> Int32 {
    cliPrint("export DOCKER_HOST=unix://\(HarpoonPaths.dockerSocketPath)")
    return 0
}

func handleExec(args: [String]) -> Int32 {
    // parse -- separator
    var argv: [String] = []
    if let dashIdx = args.firstIndex(of: "--") {
        argv = Array(args[(dashIdx+1)...])
    } else {
        // allow without -- if args provided (support both)
        argv = args
        // but if args empty or starts with -, treat as missing
        if argv.isEmpty {
            cliError("usage: harpoon exec -- <command> [args...]")
            return 2
        }
        if argv.first?.hasPrefix("-") == true {
            cliError("usage: harpoon exec -- <command> [args...]")
            cliError("hint: use '--' to separate harpoon options from guest command")
            return 2
        }
    }
    if argv.isEmpty {
        cliError("usage: harpoon exec -- <command> [args...]")
        return 2
    }
    return mgmtExec(argv: argv)
}

func handleShell(args: [String]) -> Int32 {
    // any args after shell are ignored? spec says no args
    if args.contains("--help") || args.contains("-h") {
        cliPrint("usage: harpoon shell")
        return 0
    }
    let snap = statusSnapshot()
    if snap.state == "stopped" || snap.state == "stale" {
        cliError("Harpoon VM is not running")
        return 1
    }
    if !waitForManagementReady() {
        cliError("Guest management service is not ready: \(managementFailureReason())")
        return 1
    }
    guard let fd = connectMgmtSocket() else {
        cliError("Connection to guest management service failed")
        return 1
    }
    // send shell request
    let req: [String: Any] = ["op": "shell"]
    guard let jsonData = try? JSONSerialization.data(withJSONObject: req, options: []),
          let jsonStr = String(data: jsonData, encoding: .utf8) else {
        close(fd); return 1
    }
    let line = jsonStr + "\n"
    let bytes = Array(line.utf8)
    var off = 0
    while off < bytes.count {
        let n = bytes.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off), bytes.count - off) }
        if n <= 0 { if errno == EINTR { continue }; close(fd); cliError("Connection to guest management service failed"); return 1 }
        off += Int(n)
    }
    // terminal raw mode if tty
    let isTTY = isatty(STDIN_FILENO) != 0
    var origTerm = termios()
    var rawTerm = termios()
    var didSetRaw = false
    if isTTY {
        if tcgetattr(STDIN_FILENO, &origTerm) == 0 {
            rawTerm = origTerm
            cfmakeraw(&rawTerm)
            // keep SIG handling? raw disables, we want Ctrl-C to go to guest, so keep raw
            if tcsetattr(STDIN_FILENO, TCSANOW, &rawTerm) == 0 {
                didSetRaw = true
            }
        }
    }
    defer {
        if didSetRaw { tcsetattr(STDIN_FILENO, TCSANOW, &origTerm) }
        close(fd)
    }
    // proxy loop: stdin <-> socket, socket <-> stdout
    signal(SIGPIPE, SIG_IGN)
    var shouldExit = false
    var receivedGuestData = false
    var connectionFailed = false
    // set non-blocking for stdin and fd
    let f1 = fcntl(STDIN_FILENO, F_GETFL, 0); if f1 >= 0 { _ = fcntl(STDIN_FILENO, F_SETFL, f1 | O_NONBLOCK) }
    let f2 = fcntl(fd, F_GETFL, 0); if f2 >= 0 { _ = fcntl(fd, F_SETFL, f2 | O_NONBLOCK) }
    while !shouldExit {
        var pfds = [pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0), pollfd(fd: fd, events: Int16(POLLIN), revents: 0)]
        let ret = poll(&pfds, 2, 1000)
        if ret < 0 { if errno == EINTR { continue }; connectionFailed = true; break }
        if ret == 0 { continue }
        if (pfds[0].revents & Int16(POLLIN)) != 0 {
            var buf = [UInt8](repeating: 0, count: 8192)
            let n = read(STDIN_FILENO, &buf, buf.count)
            if n > 0 {
                var off2 = 0
                while off2 < n {
                    let w = buf.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off2), n-off2) }
                    if w > 0 { off2 += Int(w); continue }
                    if w < 0 && errno == EINTR { continue }
                    if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                    connectionFailed = true; shouldExit = true; break
                }
            } else if n == 0 {
                // EOF
                shutdown(fd, SHUT_WR)
                shouldExit = true
            }
        }
        if (pfds[1].revents & Int16(POLLIN)) != 0 {
            var buf = [UInt8](repeating: 0, count: 8192)
            let n = buf.withUnsafeMutableBytes { ptr in read(fd, ptr.baseAddress!, ptr.count) }
            if n > 0 {
                receivedGuestData = true
                var off2 = 0
                while off2 < n {
                    let w = buf.withUnsafeBytes { ptr in write(STDOUT_FILENO, ptr.baseAddress!.advanced(by: off2), n-off2) }
                    if w > 0 { off2 += Int(w); continue }
                    if w < 0 && errno == EINTR { continue }
                    if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                    break
                }
            } else if n == 0 {
                if !receivedGuestData { connectionFailed = true }
                break
            } else {
                if errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR {
                    connectionFailed = true
                    break
                }
            }
        }
    }
    if connectionFailed {
        cliError("Guest management connection closed before shell started")
        return 1
    }
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
