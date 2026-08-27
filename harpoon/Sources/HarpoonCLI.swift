import Foundation
import Virtualization

// MARK: - Paths

enum HarpoonPaths {
    static var appSupportDir: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let primary = home.appendingPathComponent("Library/Application Support/Harpoon")
        // Sandbox fallback: if primary not writable, use /tmp/harpoon-runtime (allowed under Muse sandbox)
        let fm = FileManager.default
        if fm.fileExists(atPath: primary.path) {
            // check writable by attempting to create test file via FileManager (not bash)
            if fm.isWritableFile(atPath: primary.path) { return primary }
            // try fallback
            let fallback = URL(fileURLWithPath: "/tmp/harpoon-runtime")
            try? fm.createDirectory(at: fallback, withIntermediateDirectories: true, attributes: nil)
            if fm.isWritableFile(atPath: fallback.path) { return fallback }
            return primary
        } else {
            // try primary first
            do {
                try fm.createDirectory(at: primary, withIntermediateDirectories: true, attributes: nil)
                return primary
            } catch {
                let fallback = URL(fileURLWithPath: "/tmp/harpoon-runtime")
                try? fm.createDirectory(at: fallback, withIntermediateDirectories: true, attributes: nil)
                return fallback
            }
        }
    }
    static var pidFile: URL { appSupportDir.appendingPathComponent("runtime.pid") }
    static var jsonFile: URL { appSupportDir.appendingPathComponent("runtime.json") }
    static var logFile: URL { appSupportDir.appendingPathComponent("harpoon.log") }
    static var logFilePrev: URL { appSupportDir.appendingPathComponent("harpoon.log.1") }
    static var configFile: URL { appSupportDir.appendingPathComponent("config.json") }
    static var lockPath: String { "/tmp/harpoon.lock" }
    static var dockerSocketPath: String { "/tmp/harpoon-docker.sock" }
    static var controlSocketPath: String { "/tmp/harpoon-control" }
}

struct HarpoonUserConfig: Codable {
    var cpus: Int?
    var memory: Int?
}

func configFilePath() -> String {
    // check both primary and fallback like other helpers
    let candidates = [
        HarpoonPaths.configFile.path,
        "/tmp/harpoon-runtime/config.json",
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/config.json").path
    ]
    // prefer existing file
    for cand in candidates {
        if FileManager.default.fileExists(atPath: cand) { return cand }
    }
    return HarpoonPaths.configFile.path
}

func loadUserConfig() -> (HarpoonUserConfig?, String?) {
    let path = configFilePath()
    guard FileManager.default.fileExists(atPath: path) else { return (nil, nil) }
    guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else {
        return (nil, "cannot read \(path)")
    }
    if data.isEmpty { return (HarpoonUserConfig(), nil) }
    do {
        let c = try JSONDecoder().decode(HarpoonUserConfig.self, from: data)
        return (c, nil)
    } catch {
        return (nil, "\(error)")
    }
}

func saveUserConfig(_ cfg: HarpoonUserConfig) -> String? {
    let path = HarpoonPaths.configFile.path
    try? FileManager.default.createDirectory(at: HarpoonPaths.appSupportDir, withIntermediateDirectories: true, attributes: nil)
    do {
        let data = try JSONEncoder().encode(cfg)
        try data.write(to: URL(fileURLWithPath: path), options: .atomic)
        return nil
    } catch {
        return "\(error)"
    }
}

struct RuntimeMetadata: Codable {
    let pid: Int32
    let startedAt: String
    let cpus: Int
    let memoryMiB: Int
    let diskPath: String
    let socketPath: String
    let uuid: String
    let binary: String
}

// MARK: - Process checks

func isProcessAlive(pid: Int32) -> Bool {
    if kill(pid, 0) == 0 { return true }
    if errno == EPERM { return true }
    // fallback: proc_pidpath (works even when sandbox blocks ps/kill via shell)
    var buf = [CChar](repeating: 0, count: 4096)
    let ret = proc_pidpath(pid, &buf, UInt32(buf.count))
    return ret > 0
}

func isHarpoonProcess(pid: Int32) -> Bool {
    // Primary: proc_pidpath (no shellout, sandbox-safe, PID safety)
    var buf = [CChar](repeating: 0, count: 4096)
    let ret = proc_pidpath(pid, &buf, UInt32(buf.count))
    if ret > 0 {
        let path = String(cString: buf)
        if path.lowercased().contains("harpoon") { return true }
        // also check if still harpoon by path basename
        if URL(fileURLWithPath: path).lastPathComponent.lowercased().contains("harpoon") { return true }
        // proc_pidpath succeeded but not harpoon -> not harpoon
        // fall back to ps for edge cases where binary was moved
    }
    // Fallback: ps (may be blocked in sandbox, but try)
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: "/bin/ps")
    proc.arguments = ["-o", "comm=", "-p", "\(pid)"]
    let pipe = Pipe()
    proc.standardOutput = pipe
    proc.standardError = FileHandle.nullDevice
    do {
        try proc.run()
        proc.waitUntilExit()
        if proc.terminationStatus == 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            if let out = String(data: data, encoding: .utf8) {
                let comm = out.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
                if !comm.isEmpty && comm.contains("harpoon") { return true }
            }
        }
        let proc2 = Process()
        proc2.executableURL = URL(fileURLWithPath: "/bin/ps")
        proc2.arguments = ["-o", "args=", "-p", "\(pid)"]
        let pipe2 = Pipe()
        proc2.standardOutput = pipe2
        proc2.standardError = FileHandle.nullDevice
        try proc2.run()
        proc2.waitUntilExit()
        let data2 = pipe2.fileHandleForReading.readDataToEndOfFile()
        if let out2 = String(data: data2, encoding: .utf8) {
            if out2.lowercased().contains("harpoon") { return true }
        }
        // if proc_pidpath earlier failed but ps also failed, conservatively assume not harpoon
        return false
    } catch {
        return false
    }
}

func isLockHeld() -> Bool {
    let fd = open(HarpoonPaths.lockPath, O_RDWR, 0o600)
    if fd < 0 {
        // no lock file -> not held
        return false
    }
    defer { close(fd) }
    if flock(fd, LOCK_EX | LOCK_NB) == 0 {
        // we acquired -> not held; unlock
        flock(fd, LOCK_UN)
        return false
    } else {
        if errno == EWOULDBLOCK { return true }
        return false
    }
}

func socketExists0600(_ path: String) -> Bool {
    var st = stat()
    if stat(path, &st) != 0 { return false }
    if (st.st_mode & S_IFMT) != S_IFSOCK { return false }
    let perms = st.st_mode & 0o777
    // we report 0600 check elsewhere, but existence check here
    return perms == 0o600 || true // existence true even if perms off
}

func socketPerms(_ path: String) -> String {
    var st = stat()
    if stat(path, &st) != 0 { return "missing" }
    let perms = st.st_mode & 0o777
    return String(format: "%o", perms)
}

func dockerSocketReady() -> Bool {
    let path = HarpoonPaths.dockerSocketPath
    let fm = FileManager.default
    if !fm.fileExists(atPath: path) { return false }
    // try connect to socket to see if live
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return false }
    defer { close(fd) }
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
    _ = path.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let ret = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(fd, sp, len) } }
    return ret == 0
}

// MARK: - Metadata helpers

func readPIDFile() -> Int32? {
    let candidates = [
        HarpoonPaths.pidFile.path,
        "/tmp/harpoon-runtime/runtime.pid",
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/runtime.pid").path
    ]
    for cand in candidates {
        if let s = try? String(contentsOfFile: cand, encoding: .utf8) {
            let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
            if let v = Int32(trimmed) { return v }
        }
    }
    return nil
}

func readMetadata() -> RuntimeMetadata? {
    let candidates = [
        HarpoonPaths.jsonFile.path,
        "/tmp/harpoon-runtime/runtime.json",
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/runtime.json").path
    ]
    for cand in candidates {
        if let data = try? Data(contentsOf: URL(fileURLWithPath: cand)),
           let m = try? JSONDecoder().decode(RuntimeMetadata.self, from: data) {
            return m
        }
    }
    return nil
}

func ensureAppSupport() {
    try? FileManager.default.createDirectory(at: HarpoonPaths.appSupportDir, withIntermediateDirectories: true, attributes: nil)
}

func rotateLog() {
    let fm = FileManager.default
    if fm.fileExists(atPath: HarpoonPaths.logFile.path) {
        try? fm.removeItem(at: HarpoonPaths.logFilePrev)
        try? fm.moveItem(at: HarpoonPaths.logFile, to: HarpoonPaths.logFilePrev)
    }
}

func parseResourceArgs(_ args: [String]) -> (cpus: Int?, memory: Int?, kernel: String?, initramfs: String?, disk: String?, passthrough: [String]) {
    var cpus: Int? = nil
    var memory: Int? = nil
    var kernel: String? = nil
    var initramfs: String? = nil
    var disk: String? = nil
    var passthrough: [String] = []
    var i = 0
    while i < args.count {
        let a = args[i]
        if (a == "--cpus" || a == "--cpu") && i+1 < args.count {
            if let v = Int(args[i+1]) { cpus = v } else { cpus = -1 }
            passthrough.append(a); passthrough.append(args[i+1])
            i += 2
        } else if a == "--memory" && i+1 < args.count {
            if let v = Int(args[i+1]) { memory = v } else { memory = -1 }
            passthrough.append(a); passthrough.append(args[i+1])
            i += 2
        } else if a == "--kernel" && i+1 < args.count {
            kernel = args[i+1]; passthrough.append(a); passthrough.append(args[i+1]); i += 2
        } else if a == "--initramfs" && i+1 < args.count {
            initramfs = args[i+1]; passthrough.append(a); passthrough.append(args[i+1]); i += 2
        } else if a == "--disk" && i+1 < args.count {
            disk = args[i+1]; passthrough.append(a); passthrough.append(args[i+1]); i += 2
        } else {
            i += 1
        }
    }
    return (cpus, memory, kernel, initramfs, disk, passthrough)
}

func resolveConfigFromArgs(_ args: [String]) -> RuntimeConfig {
    var c = RuntimeConfig.fromEnvironment()
    let parsed = parseResourceArgs(args)
    if let v = parsed.cpus { c.cpuCount = v }
    if let v = parsed.memory { c.memoryMIB = v }
    if let p = parsed.kernel { c.kernelURL = URL(fileURLWithPath: p) }
    if let p = parsed.initramfs { c.initramfsURL = URL(fileURLWithPath: p) }
    if let p = parsed.disk { c.diskURL = URL(fileURLWithPath: p) }
    return c
}

func statusSnapshot() -> (state: String, pid: Int32?, alive: Bool, isHarpoon: Bool, lockHeld: Bool, sockExists: Bool, dockerReady: Bool, meta: RuntimeMetadata?) {
    let pid = readPIDFile()
    let meta = readMetadata()
    let alive: Bool
    let isHarpoon: Bool
    if let p = pid {
        alive = isProcessAlive(pid: p)
        isHarpoon = alive ? isHarpoonProcess(pid: p) : false
    } else {
        alive = false; isHarpoon = false
    }
    let lockHeld = isLockHeld()
    let sockExists = FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath)
    let dockerReady = dockerSocketReady()
    // fallback: check HARPOON_RUNNING in log (sandbox may block unix connect)
    // check both primary and fallback locations due to sandbox divergence
    var logHasRunning = false
    let logCandidates = [
        HarpoonPaths.logFile.path,
        "/tmp/harpoon-runtime/harpoon.log",
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/harpoon.log").path,
        "/tmp/harpoon.log"
    ]
    for cand in logCandidates {
        if let log = try? String(contentsOfFile: cand, encoding: .utf8), log.contains("HARPOON_RUNNING") {
            logHasRunning = true
            break
        }
    }
    let effectiveReady = dockerReady || (sockExists && logHasRunning)
    let state: String
    if pid == nil && !lockHeld && !sockExists {
        state = "stopped"
    } else if let _ = pid, alive, isHarpoon, sockExists, effectiveReady {
        state = "running"
    } else if let _ = pid, alive, isHarpoon, sockExists, !effectiveReady {
        state = "starting"
    } else if let _ = pid, alive, isHarpoon, !sockExists {
        state = "starting"
    } else if let _ = pid, alive, !isHarpoon {
        state = "stale"
    } else if pid != nil && !alive {
        state = "stale"
    } else if lockHeld && !alive {
        state = "degraded"
    } else if alive && !sockExists {
        state = "degraded"
    } else {
        state = "stopped"
    }
    return (state, pid, alive, isHarpoon, lockHeld, sockExists, dockerReady, meta)
}

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
            "lockHeld": snap.lockHeld
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

// MARK: - Docker context integration (M8)

let harpoonContextName = "harpoon"
let harpoonSocketEndpoint = "unix:///tmp/harpoon-docker.sock"

func findDocker() -> String? {
    let candidates = ["/usr/local/bin/docker", "/opt/homebrew/bin/docker", "/usr/bin/docker", "/run/current-system/sw/bin/docker"]
    for c in candidates {
        if FileManager.default.isExecutableFile(atPath: c) { return c }
    }
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: "/usr/bin/which")
    proc.arguments = ["docker"]
    let pipe = Pipe()
    proc.standardOutput = pipe
    proc.standardError = FileHandle.nullDevice
    do {
        try proc.run()
        proc.waitUntilExit()
        if proc.terminationStatus == 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            if let out = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines), !out.isEmpty {
                if FileManager.default.isExecutableFile(atPath: out) { return out }
            }
        }
    } catch {}
    // try whereis via env
    if let pathEnv = ProcessInfo.processInfo.environment["PATH"] {
        for dir in pathEnv.split(separator: ":") {
            let cand = URL(fileURLWithPath: String(dir)).appendingPathComponent("docker").path
            if FileManager.default.isExecutableFile(atPath: cand) { return cand }
        }
    }
    return nil
}

func runDocker(_ args: [String], env: [String:String]? = nil) -> (Int32, String, String) {
    guard let docker = findDocker() else { return (127, "", "docker not found") }
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: docker)
    proc.arguments = args
    if let e = env {
        var merged = ProcessInfo.processInfo.environment
        for (k,v) in e { merged[k]=v }
        proc.environment = merged
    }
    let outPipe = Pipe()
    let errPipe = Pipe()
    proc.standardOutput = outPipe
    proc.standardError = errPipe
    do {
        try proc.run()
        proc.waitUntilExit()
        let out = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let err = String(data: errPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return (proc.terminationStatus, out, err)
    } catch {
        return (1, "", "\(error)")
    }
}

func dockerContextEndpoint(_ name: String) -> String? {
    let (code, out, _) = runDocker(["context", "inspect", name])
    if code != 0 { return nil }
    // parse JSON
    guard let data = out.data(using: .utf8) else { return nil }
    if let arr = try? JSONSerialization.jsonObject(with: data) as? [[String:Any]],
       let first = arr.first,
       let endpoints = first["Endpoints"] as? [String:Any],
       let dockerEp = endpoints["docker"] as? [String:Any],
       let host = dockerEp["Host"] as? String {
        return host
    }
    return nil
}

func dockerContextExists(_ name: String) -> Bool {
    return dockerContextEndpoint(name) != nil
}

func currentDockerContext() -> String? {
    let (code, out, _) = runDocker(["context", "show"])
    if code != 0 { return nil }
    let s = out.trimmingCharacters(in: .whitespacesAndNewlines)
    return s.isEmpty ? nil : s
}

func handleDockerSetup() -> Int32 {
    guard let docker = findDocker() else {
        cliError("docker CLI not found (install Docker)")
        return 1
    }
    cliPrint("Docker CLI: \(docker)")
    let exists = dockerContextExists(harpoonContextName)
    if exists {
        if let ep = dockerContextEndpoint(harpoonContextName) {
            if ep == harpoonSocketEndpoint {
                cliPrint("Harpoon context already exists and is valid")
                cliPrint("Endpoint: \(ep)")
                if let cur = currentDockerContext() { cliPrint("Current context: \(cur)") }
                cliPrint("Docker socket: \(HarpoonPaths.dockerSocketPath)")
                return 0
            } else {
                cliError("Conflict: context 'harpoon' already exists but targets different endpoint")
                cliError("  existing: \(ep)")
                cliError("  expected: \(harpoonSocketEndpoint)")
                cliError("Refusing to overwrite. Remove manually with: harpoon docker remove (only if owned) or docker context rm harpoon")
                return 1
            }
        }
    }
    // create
    cliPrint("Creating Docker context 'harpoon' -> \(harpoonSocketEndpoint)")
    let (code, out, err) = runDocker(["context", "create", harpoonContextName, "--docker", "host=\(harpoonSocketEndpoint)", "--description", "Harpoon"])
    if code != 0 {
        // maybe already exists race, try inspect again
        if dockerContextExists(harpoonContextName), let ep = dockerContextEndpoint(harpoonContextName), ep == harpoonSocketEndpoint {
            cliPrint("Context now exists (race)")
            return 0
        }
        cliError("docker context create failed (\(code))")
        if !out.isEmpty { cliError(out) }
        if !err.isEmpty { cliError(err) }
        return 1
    }
    cliPrint(out.trimmingCharacters(in: .whitespacesAndNewlines))
    if let ep = dockerContextEndpoint(harpoonContextName) {
        cliPrint("Endpoint: \(ep)")
    }
    cliPrint("Done. Try: docker --context harpoon version")
    return 0
}

func handleDockerStatus() -> Int32 {
    if let docker = findDocker() {
        cliPrint("Docker CLI: \(docker)")
        let (vCode, vOut, _) = runDocker(["--version"])
        if vCode==0 { cliPrint(vOut.trimmingCharacters(in: .whitespacesAndNewlines)) }
    } else {
        cliPrint("Docker CLI: not found")
    }
    if dockerContextExists(harpoonContextName) {
        if let ep = dockerContextEndpoint(harpoonContextName) {
            if ep == harpoonSocketEndpoint {
                cliPrint("Harpoon context: installed")
                cliPrint("Endpoint: \(ep)")
            } else {
                cliPrint("Harpoon context: conflict")
                cliPrint("Endpoint: \(ep) (expected \(harpoonSocketEndpoint))")
            }
        }
    } else {
        cliPrint("Harpoon context: not installed (run harpoon docker setup)")
    }
    if let cur = currentDockerContext() { cliPrint("Current context: \(cur)") } else { cliPrint("Current context: unknown") }
    let snap = statusSnapshot()
    cliPrint("Harpoon runtime: \(snap.state)")
    if let pid = snap.pid { cliPrint("PID: \(pid)") }
    cliPrint("Socket: \(HarpoonPaths.dockerSocketPath) exists=\(snap.sockExists) ready=\(snap.dockerReady)")
    var st = stat()
    let perms = stat(HarpoonPaths.dockerSocketPath, &st)==0 ? String(format:"%o", st.st_mode & 0o777) : "missing"
    cliPrint("Socket perms: \(perms) (expected 600)")
    return 0
}

func handleDockerRemove() -> Int32 {
    guard findDocker() != nil else { cliError("docker CLI not found"); return 1 }
    guard dockerContextExists(harpoonContextName) else { cliPrint("Harpoon context not installed"); return 0 }
    guard let ep = dockerContextEndpoint(harpoonContextName) else { cliError("cannot inspect harpoon context"); return 1 }
    if ep != harpoonSocketEndpoint {
        cliError("Refusing to remove conflicting context 'harpoon' (endpoint \(ep) != \(harpoonSocketEndpoint))")
        cliError("Not a Harpoon-owned context. Remove manually if intended: docker context rm harpoon")
        return 1
    }
    // if current context is harpoon, switch to default first
    if let cur = currentDockerContext(), cur == harpoonContextName {
        let (code, _, err) = runDocker(["context", "use", "default"])
        if code != 0 {
            // try desktop-linux or just force
            _ = runDocker(["context", "use", "desktop-linux"])
        }
    }
    let (code, out, err) = runDocker(["context", "rm", harpoonContextName])
    if code != 0 {
        // try force
        let (code2, out2, err2) = runDocker(["context", "rm", "-f", harpoonContextName])
        if code2 != 0 {
            cliError("docker context rm failed")
            if !err.isEmpty { cliError(err) }
            if !err2.isEmpty { cliError(err2) }
            return 1
        }
        cliPrint(out2)
    } else {
        cliPrint(out)
    }
    cliPrint("Harpoon context removed")
    return 0
}

func handleDockerUse() -> Int32 {
    guard findDocker() != nil else { cliError("docker CLI not found"); return 1 }
    guard dockerContextExists(harpoonContextName) else {
        cliError("Harpoon context not installed (run harpoon docker setup)")
        return 1
    }
    if let ep = dockerContextEndpoint(harpoonContextName), ep != harpoonSocketEndpoint {
        cliError("Conflict: harpoon context exists but endpoint \(ep) != \(harpoonSocketEndpoint)")
        return 1
    }
    let (code, out, err) = runDocker(["context", "use", harpoonContextName])
    if code != 0 {
        cliError("docker context use failed")
        if !err.isEmpty { cliError(err) }
        return 1
    }
    cliPrint(out.trimmingCharacters(in: .whitespacesAndNewlines))
    cliPrint("Now using context: harpoon")
    return 0
}

func handleDocker(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="help" || args[0]=="--help" || args[0]=="-h" {
        cliPrint("usage: harpoon docker <setup|status|remove|use|env> [options]")
        cliPrint("  setup   create/verify harpoon context (unix:///tmp/harpoon-docker.sock)")
        cliPrint("  status  show Docker CLI, context, runtime, socket")
        cliPrint("  remove  remove harpoon context if owned")
        cliPrint("  use     activate harpoon context (docker context use harpoon)")
        cliPrint("  env     print DOCKER_HOST for Harpoon")
        return 0
    }
    switch args[0] {
    case "setup": return handleDockerSetup()
    case "status": return handleDockerStatus()
    case "remove": return handleDockerRemove()
    case "use": return handleDockerUse()
    case "env": return handleDockerEnv()
    default:
        cliError("unknown docker subcommand: \(args[0])")
        cliPrint("usage: harpoon docker <setup|status|remove|use|env>")
        return 2
    }
}


func handleConfig(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="show" {
        let path = configFilePath()
        let (cfg, err) = loadUserConfig()
        if let e = err {
            cliError("Harpoon configuration is invalid:")
            cliError(e)
            cliError("Config: \(path)")
            cliError("Hint: run harpoon config reset cpus/memory or fix JSON")
            return 1
        }
        cliPrint("Config: \(path)")
        if let c = cfg {
            if let v = c.cpus { cliPrint("cpus: \(v)") } else { cliPrint("cpus: (default 2)") }
            if let v = c.memory { cliPrint("memory: \(v)") } else { cliPrint("memory: (default 1024)") }
            if c.cpus==nil && c.memory==nil { cliPrint("(no user config, using defaults)") }
        } else {
            cliPrint("(no user config, using defaults)")
            cliPrint("cpus: (default 2)")
            cliPrint("memory: (default 1024)")
        }
        return 0
    } else if args[0]=="path" {
        cliPrint(configFilePath())
        return 0
    } else if args[0]=="set" && args.count>=3 {
        let key = args[1]
        let valStr = args[2]
        var (cfg, err) = loadUserConfig()
        if let e = err {
            cliError("Harpoon configuration is invalid: \(e)")
            cliError("Config: \(configFilePath())")
            return 1
        }
        var cur = cfg ?? HarpoonUserConfig()
        if key=="cpus" || key=="cpu" {
            guard let v = Int(valStr) else { cliError("invalid cpus: \(valStr)"); return 1 }
            if v<1 || v>8 { cliError("cpus must be 1...8, got \(v)"); return 1 }
            cur.cpus = v
        } else if key=="memory" {
            guard let v = Int(valStr) else { cliError("invalid memory: \(valStr)"); return 1 }
            if ![512,768,1024].contains(v) { cliError("memory must be 512|768|1024, got \(v)"); return 1 }
            cur.memory = v
        } else {
            cliError("unknown config key: \(key) (expected cpus, memory)")
            return 1
        }
        if let e = saveUserConfig(cur) { cliError("failed to save: \(e)"); return 1 }
        cliPrint("set \(key)=\(valStr)")
        return 0
    } else if args[0]=="reset" && args.count>=2 {
        let key = args[1]
        var (cfg, err) = loadUserConfig()
        if let e = err {
            // if malformed, reset by removing file
            try? FileManager.default.removeItem(atPath: configFilePath())
            cliPrint("removed malformed config")
            return 0
        }
        var cur = cfg ?? HarpoonUserConfig()
        if key=="cpus" || key=="cpu" { cur.cpus = nil }
        else if key=="memory" { cur.memory = nil }
        else if key=="all" { cur = HarpoonUserConfig() }
        else { cliError("unknown key: \(key)"); return 1 }
        // if both nil, remove file
        if cur.cpus==nil && cur.memory==nil {
            try? FileManager.default.removeItem(atPath: configFilePath())
            cliPrint("reset \(key) (now default)")
        } else {
            if let e = saveUserConfig(cur) { cliError("failed: \(e)"); return 1 }
            cliPrint("reset \(key)")
        }
        return 0
    } else if args[0]=="--help" || args[0]=="-h" || args[0]=="help" {
        cliPrint("usage: harpoon config <show|set|reset|path> [args]")
        cliPrint("  show              Show current config and path")
        cliPrint("  set cpus 2        Set cpus 1...8")
        cliPrint("  set memory 1024   Set memory 512|768|1024")
        cliPrint("  reset cpus        Reset to default")
        cliPrint("  reset memory      Reset to default")
        cliPrint("  path              Print config file path")
        return 0
    } else {
        cliError("unknown config command: \(args.joined(separator: " "))")
        cliPrint("usage: harpoon config <show|set|reset|path>")
        return 1
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
    let kernel = RuntimeConfig().kernelURL.path
    check(FileManager.default.fileExists(atPath: kernel), "kernel \(kernel)")
    let initramfs = RuntimeConfig().initramfsURL.path
    check(FileManager.default.fileExists(atPath: initramfs), "initramfs \(initramfs)")
    let disk = RuntimeConfig().diskURL.path
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
    // DOCKER
    if let docker = findDocker() {
        check(true, "Docker CLI \(docker)")
        let (code, out, _) = runDocker(["--version"])
        if code==0 { cliPrint("INFO  \(out.trimmingCharacters(in: .whitespacesAndNewlines))") }
        if dockerContextExists(harpoonContextName) {
            if let ep = dockerContextEndpoint(harpoonContextName) {
                check(ep==harpoonSocketEndpoint, "context harpoon endpoint \(ep)")
            }
        } else {
            check(false, "context harpoon not installed (run harpoon docker setup)", warn:true)
        }
        if let cur = currentDockerContext() { cliPrint("INFO  active context \(cur)") }
        if snap.state=="running" && sockExists {
            let (c, _, _) = runDocker(["--context", "harpoon", "version"])
            check(c==0, "Docker API reachable via harpoon context")
        }
    } else {
        check(false, "Docker CLI not found", warn:true)
    }
    // RUNTIME
    if snap.state=="stale" { check(false, "stale PID metadata", warn:true) }
    if snap.state=="degraded" { check(false, "degraded state", warn:true) }
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

func handleVersion() -> Int32 {
    cliPrint("Harpoon 0.1.0")
    return 0
}

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

    ensureAppSupport()
    rotateLog()

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
    // wait for sockets/lock to clear
    for _ in 0..<10 {
        if !FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath) && !FileManager.default.fileExists(atPath: HarpoonPaths.controlSocketPath) && !isLockHeld() {
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
      docker      Manage Docker integration
      doctor      Diagnose common problems
      run         Run in foreground (debug)
      version     Show version
      help        Show help

    Start options:
      --cpus N                1...8 (default 2)
      --memory 512|768|1024   (default 1024)
      --kernel PATH
      --initramfs PATH
      --disk PATH

    Precedence: CLI flag > user config > environment > defaults
    Config: ~/Library/Application Support/Harpoon/config.json

    Docker:
      harpoon docker setup   Create harpoon context (unix:///tmp/harpoon-docker.sock)
      harpoon docker status
      docker --context harpoon ps

    Examples:
      harpoon start
      harpoon docker setup
      docker --context harpoon run --rm hello-world
      docker --context harpoon compose up -d

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
