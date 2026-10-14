import Foundation
import Darwin

private let diagnosticsSchema = "harpoon.diagnostics.snapshot.v1"

private func metric(_ name: String, _ value: Any?, _ unit: String, _ scope: String, _ domain: String, _ source: String, status: String = "observed", reliability: String = "reliable") -> [String: Any] {
    var m: [String: Any] = ["name": name, "unit": unit, "scope": scope, "domain": domain, "source": source, "status": status, "reliability": reliability]
    if let value { m["value"] = value }
    return m
}

private func command(_ path: String, _ args: [String], limit: Int = 256 * 1024) -> String {
    let p = Process(); p.executableURL = URL(fileURLWithPath: path); p.arguments = args
    let pipe = Pipe(); p.standardOutput = pipe; p.standardError = FileHandle.nullDevice
    do { try p.run(); p.waitUntilExit() } catch { return "" }
    let data = pipe.fileHandleForReading.readData(ofLength: limit)
    return String(data: data, encoding: .utf8) ?? ""
}

private func hostProcesses(_ pid: Int?) -> [[String: Any]] {
    guard let pid else { return [] }
    let raw = command("/bin/ps", ["-p", "\(pid)", "-o", "pid=,ppid=,state=,rss=,pcpu=,command="])
    let fields = raw.split(separator: "\n").first?.split(maxSplits: 5, omittingEmptySubsequences: true, whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init) ?? []
    guard fields.count >= 6 else { return [] }
    return [[
        "pid": Int(fields[0]) ?? pid, "ppid": Int(fields[1]) ?? 0, "state": fields[2],
        "metrics": [metric("rss", Int(fields[3]), "KiB", "host-process", "HOST", "ps -p <harpoon-pid>"), metric("cpu.percent", Double(fields[4]) ?? 0, "percent", "host-process", "HOST", "ps -p <harpoon-pid>", reliability: "best-effort")],
        "cmdline": fields[5]
    ]]
}

private func guestProbe() -> (memory: [[String: Any]], processes: [[String: Any]], files: [String: [String]], pids: [String: String]) {
    let script = """
    printf 'MEMINFO\\n'; cat /proc/meminfo
    printf 'PROCESSES\\n'; ps -eo pid,ppid,stat,rss,args 2>/dev/null | head -n 257
    printf 'CGROUPS\\n'; find /sys/fs/cgroup -maxdepth 5 -type f \\( -name memory.current -o -name memory.stat -o -name cpu.stat -o -name pids.current -o -name io.stat \\) -print 2>/dev/null | head -n 257 | while read f; do printf 'FILE %s\\n' "$f"; head -n 64 "$f"; done
    printf 'PIDS\\n'; for p in $(ps -eo pid= 2>/dev/null | head -n 256); do printf 'PID %s\\n' "$p"; head -n 8 /proc/$p/cgroup 2>/dev/null; done
    """
    let result = mgmtExecCapture(argv: ["sh", "-c", script])
    guard result.code == 0 else { return ([], [], [:], [:]) }
    var section = ""
    var memory: [[String: Any]] = [], processes: [[String: Any]] = [], files: [String: [String]] = [:], pids: [String: String] = [:], currentFile: String?
    for line in result.stdout.split(separator: "\n", omittingEmptySubsequences: false).map(String.init) {
        if ["MEMINFO", "PROCESSES", "CGROUPS", "PIDS"].contains(line) { section = line; currentFile = nil; continue }
        if section == "MEMINFO", let colon = line.firstIndex(of: ":") {
            let name = String(line[..<colon]); let parts = line[line.index(after: colon)...].split(whereSeparator: { $0 == " " || $0 == "\t" })
            if let n = parts.first, let value = Int(n) { memory.append(metric(name, value, parts.dropFirst().first.map(String.init) ?? "kB", "guest", "GUEST", "/proc/meminfo")) }
        } else if section == "PROCESSES", let pid = line.split(separator: " ").first, Int(pid) != nil {
            let parts = line.split(maxSplits: 4, omittingEmptySubsequences: true, whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
            if parts.count >= 5 { processes.append(["pid": Int(parts[0]) ?? 0, "ppid": Int(parts[1]) ?? 0, "state": parts[2], "metrics": [metric("rss", Int(parts[3]), "KiB", "guest-process", "GUEST", "ps -eo pid,ppid,stat,rss,args")], "cmdline": parts[4]]) }
        } else if section == "CGROUPS" {
            if line.hasPrefix("FILE ") { currentFile = String(line.dropFirst(5)); files[currentFile!] = [] }
            else if let currentFile { files[currentFile, default: []].append(line) }
        } else if section == "PIDS", line.hasPrefix("PID ") {
            let pid = String(line.dropFirst(4)); currentFile = pid; pids[pid] = ""
        } else if section == "PIDS", let currentFile { pids[currentFile, default: ""] += line + "\n" }
    }
    return (memory, processes, files, pids)
}

private func cgroupMetrics(_ path: String, files: [String: [String]]) -> [[String: Any]] {
    let names = ["memory.current", "memory.stat", "cpu.stat", "pids.current", "io.stat"]
    return names.compactMap { name in
        let key = path + "/" + name
        guard let lines = files[key], !lines.isEmpty else { return metric(name, nil, "", "workload-cgroup", "WORKLOADS", "guest cgroup v2", status: "unavailable", reliability: "not-observed") }
        if name == "memory.current" || name == "pids.current" { return metric(name, Int(lines[0].trimmingCharacters(in: .whitespacesAndNewlines)), "bytes", "workload-cgroup", "WORKLOADS", "guest cgroup v2") }
        return metric(name, lines.joined(separator: "\n"), "text", "workload-cgroup", "WORKLOADS", "guest cgroup v2", reliability: "reliable")
    }
}

func diagnosticsSnapshot() -> [String: Any] {
    let status = statusSnapshot()
    let cfg = RuntimeConfig.fromEnvironment()
    let guest = guestProbe()
    var containers: [[String: Any]] = []
    if findDocker() != nil, status.dockerReady {
        let ids = runDocker(["--context", harpoonContextName, "ps", "-aq", "--no-trunc"]).1.split(whereSeparator: { $0 == "\n" }).map(String.init).prefix(128)
        if !ids.isEmpty, let data = runDocker(["--context", harpoonContextName, "inspect"] + ids).1.data(using: .utf8), let rows = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] {
            for row in rows {
                let state = row["State"] as? [String: Any] ?? [:]; let pid = state["Pid"] as? Int ?? 0; let id = row["Id"] as? String ?? ""; let name = row["Name"] as? String ?? ""
                let cgroup = guest.pids["\(pid)"]?.split(separator: ":").last.map(String.init)?.split(separator: "/").dropFirst().map(String.init).joined(separator: "/") ?? ""
                let cgroupPath = cgroup.isEmpty ? "" : "/sys/fs/cgroup/" + cgroup
                containers.append(["identity": ["id": id, "name": name, "image": row["Config"].flatMap { ($0 as? [String: Any])?["Image"] } ?? NSNull(), "dockerAuthority": true], "pid": pid, "cgroupPath": cgroupPath.isEmpty ? NSNull() : cgroupPath, "metrics": cgroupPath.isEmpty ? [metric("cgroup", nil, "", "workload-cgroup", "WORKLOADS", "guest /proc/<pid>/cgroup", status: "unavailable", reliability: "not-observed")] : cgroupMetrics(cgroupPath, files: guest.files)])
            }
        }
    }
    let health: [String: Any] = ["vm": ["state": status.state, "source": "Harpoon lifecycle runtime.json", "confidence": "observed"], "docker": ["state": status.dockerReady ? "ready" : "unavailable", "source": "Docker Engine API probe", "confidence": status.dockerReady ? "observed" : "unavailable"], "management": ["state": isMgmtServiceReachable() ? "ready" : "unavailable", "source": "management vsock request", "confidence": "observed"]]
    return ["schema": diagnosticsSchema, "capturedAt": ISO8601DateFormatter().string(from: Date()), "semantics": ["grandTotal": "omitted: host, guest, and workload domains are not mathematically additive", "authorities": ["HOST": "Harpoon/macOS observation", "GUEST": "Linux /proc and process observation", "WORKLOADS": "Docker identity plus Linux cgroup v2 resource truth"]], "health": health, "domains": ["HOST": ["config": ["cpus": cfg.cpuCount, "memoryMiB": cfg.memoryMIB], "processes": hostProcesses(status.pid.map { Int($0) }), "vzAssociation": ["status": "unavailable", "source": "no reliable public per-VM host metric in current architecture", "confidence": "unavailable"]], "GUEST": ["memory": guest.memory, "processes": guest.processes, "runtimeProcessNames": ["dockerd", "containerd", "containerd-shim", "runc"], "source": "management vsock -> /proc and cgroup filesystem"], "WORKLOADS": ["containers": containers, "identitySource": "Docker inspect", "resourceSource": "Linux cgroup v2 files; docker stats is not used"]]]
}

func handleDiagnostics(args: [String]) -> Int32 {
    if args.contains("--help") || args.contains("-h") { cliPrint("usage: harpoon diagnostics <snapshot|explain|subscribe> [--json] [--interval SECONDS] [--count N]"); return 0 }
    let subcommand = args.first ?? "snapshot"
    if subcommand == "explain" {
        cliPrint("Diagnostics keeps HOST, GUEST, and WORKLOADS separate. Host footprint, guest /proc memory, and workload cgroup usage are not added into a grand total. Docker inspect supplies container identity and metadata; Linux cgroup v2 files supply workload resource truth. unavailable means the source could not be observed, not zero. subscribe is bounded polling over the existing collectors; no daemon or invented event stream is created.")
        return 0
    }
    var count = subcommand == "subscribe" ? 1 : 1; var interval = 1.0
    var i = 1
    while i < args.count { if args[i] == "--count", i + 1 < args.count { count = min(max(Int(args[i+1]) ?? 1, 1), 60); i += 2 } else if args[i] == "--interval", i + 1 < args.count { interval = min(max(Double(args[i+1]) ?? 1, 0.1), 60); i += 2 } else { i += 1 } }
    for n in 0..<count { let snap = diagnosticsSnapshot(); if let data = try? JSONSerialization.data(withJSONObject: snap, options: [.sortedKeys, .prettyPrinted]), let text = String(data: data, encoding: .utf8) { print(text) }; if n + 1 < count { Thread.sleep(forTimeInterval: interval) } }
    return 0
}
