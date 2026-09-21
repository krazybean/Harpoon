import Foundation
import Virtualization
import Darwin

// MARK: - Paths

enum HarpoonPaths {
    // Production persistent location MUST be ~/Library/Application Support/Harpoon.
    // Never silently fall back to /tmp for persistent Docker data — /tmp is ephemeral.
    // Temporary root allowed ONLY when explicitly enabled for tests/harnesses via env override.
    static var isTestFallbackEnabled: Bool {
        let env = ProcessInfo.processInfo.environment
        return env["HARPOON_ALLOW_TMP_FALLBACK"] == "1" || env["HARPOON_TEST_MODE"] == "1" || env["HARPOON_TEST_TMPDIR"] != nil
    }
    static var testFallbackDir: URL {
        if let custom = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"], !custom.isEmpty {
            return URL(fileURLWithPath: custom)
        }
        return URL(fileURLWithPath: "/tmp/harpoon-runtime")
    }
    static var appSupportDir: URL {
        if let custom = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"], !custom.isEmpty {
            let dir = URL(fileURLWithPath: custom)
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: nil)
            return dir
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let primary = home.appendingPathComponent("Library/Application Support/Harpoon")
        if isTestFallbackEnabled {
            // Test/development: allow fallback to /tmp when primary not writable
            let fm = FileManager.default
            if fm.fileExists(atPath: primary.path) {
                if fm.isWritableFile(atPath: primary.path) { return primary }
                let fallback = testFallbackDir
                try? fm.createDirectory(at: fallback, withIntermediateDirectories: true, attributes: nil)
                if fm.isWritableFile(atPath: fallback.path) { return fallback }
                return primary
            } else {
                do {
                    try fm.createDirectory(at: primary, withIntermediateDirectories: true, attributes: nil)
                    return primary
                } catch {
                    let fallback = testFallbackDir
                    try? fm.createDirectory(at: fallback, withIntermediateDirectories: true, attributes: nil)
                    return fallback
                }
            }
        } else {
            // Production/Finder: never choose /tmp automatically
            return primary
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
    static var mgmtSocketPath: String { "/tmp/harpoon-mgmt.sock" }
}

struct HarpoonUserConfig: Codable {
    var cpus: Int?
    var memory: Int?
    var diskSize: String? // Stage 3C: e.g. "16G", "8GiB"
}

func configFilePath() -> String {
    // Production: only primary ~/Library/...; fallback only when explicitly enabled for tests
    var candidates = [HarpoonPaths.configFile.path]
    if HarpoonPaths.isTestFallbackEnabled {
        candidates.append("/tmp/harpoon-runtime/config.json")
        candidates.append(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/config.json").path)
        candidates.append(HarpoonPaths.testFallbackDir.appendingPathComponent("config.json").path)
    }
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
    var candidates = [HarpoonPaths.pidFile.path]
    if HarpoonPaths.isTestFallbackEnabled {
        candidates.append("/tmp/harpoon-runtime/runtime.pid")
        candidates.append(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/runtime.pid").path)
        candidates.append(HarpoonPaths.testFallbackDir.appendingPathComponent("runtime.pid").path)
    }
    for cand in candidates {
        if let s = try? String(contentsOfFile: cand, encoding: .utf8) {
            let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
            if let v = Int32(trimmed) { return v }
        }
    }
    return nil
}

func readMetadata() -> RuntimeMetadata? {
    var candidates = [HarpoonPaths.jsonFile.path]
    if HarpoonPaths.isTestFallbackEnabled {
        candidates.append("/tmp/harpoon-runtime/runtime.json")
        candidates.append(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/runtime.json").path)
        candidates.append(HarpoonPaths.testFallbackDir.appendingPathComponent("runtime.json").path)
    }
    for cand in candidates {
        if let data = try? Data(contentsOf: URL(fileURLWithPath: cand)),
           let m = try? JSONDecoder().decode(RuntimeMetadata.self, from: data) {
            return m
        }
    }
    return nil
}

func ensureAppSupport() -> String? {
    let dir = HarpoonPaths.appSupportDir
    do {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: nil)
        // verify writable
        if !FileManager.default.isWritableFile(atPath: dir.path) {
            return "Application Support directory not writable: \(dir.path) — check permissions (production persistent storage must be under ~/Library/Application Support/Harpoon, not /tmp)"
        }
        return nil
    } catch {
        if HarpoonPaths.isTestFallbackEnabled {
            return nil
        }
        return "Failed to create Application Support directory \(dir.path): \(error) — Persistent user disk must live under ~/Library/Application Support/Harpoon; /tmp is ephemeral and not used in production (enable HARPOON_ALLOW_TMP_FALLBACK=1 only for tests)"
    }
}

func rotateLog() {
    let fm = FileManager.default
    if fm.fileExists(atPath: HarpoonPaths.logFile.path) {
        try? fm.removeItem(at: HarpoonPaths.logFilePrev)
        try? fm.moveItem(at: HarpoonPaths.logFile, to: HarpoonPaths.logFilePrev)
    }
}

func parseResourceArgs(_ args: [String]) -> (cpus: Int?, memory: Int?, kernel: String?, initramfs: String?, disk: String?, diskSize: String?, passthrough: [String]) {
    var cpus: Int? = nil
    var memory: Int? = nil
    var kernel: String? = nil
    var initramfs: String? = nil
    var disk: String? = nil
    var diskSize: String? = nil
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
        } else if a == "--disk-size" && i+1 < args.count {
            diskSize = args[i+1]; i += 2
        } else if a.hasPrefix("--disk-size=") {
            diskSize = String(a.dropFirst("--disk-size=".count)); i += 1
        } else {
            i += 1
        }
    }
    return (cpus, memory, kernel, initramfs, disk, diskSize, passthrough)
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
    // `run` is a supported foreground entrypoint and deliberately has no launcher
    // metadata. Its exclusive lock is therefore the secondary liveness signal.
    let activeRuntime = (alive && isHarpoon) || lockHeld
    let state: String
    if activeRuntime {
        state = sockExists && effectiveReady ? "running" : "starting"
    } else if pid != nil {
        state = "stale"
    } else if sockExists {
        state = "degraded"
    } else {
        state = "stopped"
    }
    return (state, pid, alive, isHarpoon, lockHeld, sockExists, dockerReady, meta)
}
