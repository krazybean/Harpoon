import Foundation
import Virtualization
import Darwin

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
