import Foundation
import Darwin

func isMgmtSocketLive() -> Bool {
    let path = HarpoonPaths.mgmtSocketPath
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

func connectMgmtSocket() -> Int32? {
    let path = HarpoonPaths.mgmtSocketPath
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return nil }
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
    _ = path.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let ret = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(fd, sp, len) } }
    if ret != 0 { close(fd); return nil }
    return fd
}

func managementListenerReady() -> Bool {
    guard FileManager.default.fileExists(atPath: HarpoonPaths.mgmtSocketPath),
          let log = try? String(contentsOfFile: HarpoonPaths.logFile.path, encoding: .utf8) else { return false }
    return log.contains("HARPOON_MGMT_LISTENER_READY") || log.contains("HARPOON_MGMT_READY")
}

func isMgmtServiceReachable() -> Bool {
    guard managementListenerReady(), let fd = connectMgmtSocket() else { return false }
    defer { close(fd) }
    let request = Data("{\"op\":\"exec\",\"argv\":[\"true\"]}\n".utf8)
    let wrote = request.withUnsafeBytes { write(fd, $0.baseAddress!, request.count) }
    guard wrote == request.count else { return false }
    var timeout = timeval(tv_sec: 2, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    var buffer = [UInt8](repeating: 0, count: 1024)
    let count = read(fd, &buffer, buffer.count)
    guard count > 0,
          let text = String(bytes: buffer[0..<count], encoding: .utf8),
          let line = text.split(separator: "\n").first,
          let data = line.data(using: .utf8),
          let response = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
    return (response["exit"] as? Int) == 0
}

func managementReady() -> Bool {
    isMgmtServiceReachable()
}

// Bounded request used by diagnostics collectors. It preserves argv boundaries
// and the existing management socket contract; it is not a second guest API.
func mgmtExecCapture(argv: [String], maxBytes: Int = 512 * 1024) -> (code: Int32, stdout: String, stderr: String) {
    let snap = statusSnapshot()
    guard snap.state != "stopped" && snap.state != "stale",
          waitForManagementReady(), let fd = connectMgmtSocket() else {
        return (1, "", "guest management service is not ready")
    }
    defer { close(fd) }
    let req: [String: Any] = ["op": "exec", "argv": argv]
    guard let data = try? JSONSerialization.data(withJSONObject: req, options: []) else {
        return (1, "", "failed to encode management request")
    }
    let line = Array((String(data: data, encoding: .utf8)! + "\n").utf8)
    var offset = 0
    while offset < line.count {
        let written = line.withUnsafeBytes { write(fd, $0.baseAddress!.advanced(by: offset), line.count - offset) }
        if written <= 0 { return (1, "", "management request failed") }
        offset += written
    }
    var timeout = timeval(tv_sec: 5, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    var response = Data()
    var buffer = [UInt8](repeating: 0, count: 8192)
    while response.count < maxBytes {
        let n = buffer.withUnsafeMutableBytes { read(fd, $0.baseAddress!, $0.count) }
        if n <= 0 { break }
        response.append(contentsOf: buffer[0..<n])
        if response.contains(UInt8(ascii: "\n")) { break }
    }
    guard let nl = response.firstIndex(of: UInt8(ascii: "\n")),
          let object = try? JSONSerialization.jsonObject(with: response.subdata(in: 0..<nl)) as? [String: Any] else {
        return (1, "", "invalid management response")
    }
    return ((object["exit"] as? Int).map(Int32.init) ?? 1,
            object["stdout"] as? String ?? "",
            object["stderr"] as? String ?? "")
}

func waitForManagementReady() -> Bool {
    for _ in 0..<20 {
        if managementReady() { return true }
        Thread.sleep(forTimeInterval: 0.5)
    }
    return false
}

func managementFailureReason() -> String {
    guard let log = try? String(contentsOfFile: HarpoonPaths.logFile.path, encoding: .utf8) else { return "no runtime log" }
    return log.split(separator: "\n").reversed().first { $0.contains("HARPOON_MGMT_") }.map(String.init) ?? "no guest management readiness marker"
}

func mgmtExec(argv: [String]) -> Int32 {
    let snap = statusSnapshot()
    if snap.state == "stopped" || snap.state == "stale" {
        cliError("Harpoon VM is not running")
        return 1
    }
    if snap.state == "starting" || snap.state == "degraded" {
        // still check mgmt socket live, but warn
    }
    if !waitForManagementReady() {
        cliError("Guest management service is not ready: \(managementFailureReason())")
        return 1
    }
    guard let fd = connectMgmtSocket() else {
        // distinguish: socket exists but connect failed = mgmt not ready vs connection failed
        var st = stat()
        if stat(HarpoonPaths.mgmtSocketPath, &st) != 0 {
            cliError("Guest management service is not ready")
        } else {
            cliError("Connection to guest management service failed")
        }
        return 1
    }
    defer { close(fd) }
    // send JSON line
    let req: [String: Any] = ["op": "exec", "argv": argv]
    guard let jsonData = try? JSONSerialization.data(withJSONObject: req, options: []),
          let jsonStr = String(data: jsonData, encoding: .utf8) else {
        cliError("failed to encode request")
        return 1
    }
    let line = jsonStr + "\n"
    let bytes = Array(line.utf8)
    var off = 0
    while off < bytes.count {
        let n = bytes.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off), bytes.count - off) }
        if n <= 0 {
            if errno == EINTR { continue }
            cliError("Connection to guest management service failed")
            return 1
        }
        off += Int(n)
    }
    // read response line (with timeout)
    var tv = timeval(tv_sec: 15, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    var respData = Data()
    var buf = [UInt8](repeating: 0, count: 8192)
    var foundNL = false
    while !foundNL {
        let n = buf.withUnsafeMutableBytes { ptr in read(fd, ptr.baseAddress!, ptr.count) }
        if n > 0 {
            respData.append(contentsOf: buf[0..<n])
            if respData.contains(UInt8(ascii: "\n")) { foundNL = true; break }
            if respData.count > 8*1024*1024 { break }
        } else if n == 0 {
            break
        } else {
            if errno == EAGAIN || errno == EWOULDBLOCK {
                cliError("Connection to guest management service failed")
                return 1
            }
            if errno == EINTR { continue }
            cliError("Connection to guest management service failed")
            return 1
        }
    }
    if respData.isEmpty {
        cliError("Connection to guest management service failed")
        return 1
    }
    // extract first line
    let nlIdx = respData.firstIndex(of: UInt8(ascii: "\n")) ?? respData.endIndex
    let lineData = respData.subdata(in: 0..<nlIdx)
    guard let lineStr = String(data: lineData, encoding: .utf8),
          let data = lineStr.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data, options: []) as? [String: Any] else {
        cliError("Connection to guest management service failed")
        if let s = String(data: respData, encoding: .utf8) { fputs(s, stderr) }
        return 1
    }
    let exitCode = (obj["exit"] as? Int) ?? (obj["exit"] as? Int32).map{Int($0)} ?? 1
    let stdoutStr = obj["stdout"] as? String ?? ""
    let stderrStr = obj["stderr"] as? String ?? ""
    if !stdoutStr.isEmpty { fputs(stdoutStr, stdout); if !stdoutStr.hasSuffix("\n") { fputs("\n", stdout) } }
    if !stderrStr.isEmpty { fputs(stderrStr, stderr); if !stderrStr.hasSuffix("\n") { fputs("\n", stderr) } }
    if exitCode != 0 {
        cliError("Guest command exited with status \(exitCode)")
    }
    return Int32(exitCode)
}
