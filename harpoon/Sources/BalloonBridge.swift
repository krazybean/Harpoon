import Foundation
import Virtualization

extension BridgeSet {
// MARK: - Balloon control

func startBalloonControl() {
    guard let balloon = balloonDevice else { return }
    if FileManager.default.fileExists(atPath: config.balloonControlPath) {
        if isSocketLive(config.balloonControlPath) {
            log("HARPOON_ALREADY_RUNNING balloonControl \(config.balloonControlPath) in use")
            return
        }
        log("HARPOON_STALE_CLEANUP removing stale \(config.balloonControlPath)")
        try? FileManager.default.removeItem(atPath: config.balloonControlPath)
    }
    let cfd = socket(AF_UNIX, SOCK_STREAM, 0)
    balloonControlFd = cfd
    if cfd < 0 { log("HARPOON_BALLOON_CONTROL_FAILED socket \(String(cString:strerror(errno)))"); return }
    var caddr = sockaddr_un()
    caddr.sun_family = sa_family_t(AF_UNIX)
    memset(&caddr.sun_path, 0, MemoryLayout.size(ofValue: caddr.sun_path))
    _ = config.balloonControlPath.withCString { src in withUnsafeMutablePointer(to: &caddr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
    let clen = socklen_t(MemoryLayout<sockaddr_un>.size)
    let cb = withUnsafePointer(to: caddr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in Darwin.bind(cfd, sp, clen) } }
    if cb != 0 { log("HARPOON_BALLOON_CONTROL_FAILED bind \(String(cString:strerror(errno)))"); close(cfd); balloonControlFd = -1; return }
    chmod(config.balloonControlPath, 0o600)
    if listen(cfd, 8) != 0 { log("HARPOON_BALLOON_CONTROL_FAILED listen \(String(cString:strerror(errno)))"); close(cfd); balloonControlFd = -1; return }
    ownsBalloonControlSocket = true
    let cflags = fcntl(cfd, F_GETFL, 0)
    _ = fcntl(cfd, F_SETFL, cflags | O_NONBLOCK)
    log("HARPOON_BALLOON_CONTROL_LISTENING \(config.balloonControlPath)")
    let source = DispatchSource.makeReadSource(fileDescriptor: cfd, queue: .main)
    balloonControlSource = source
    source.setEventHandler { [weak self] in
        guard let self = self else { return }
        while true {
            var ca = sockaddr_un()
            var cl: socklen_t = socklen_t(MemoryLayout<sockaddr_un>.size)
            let cfd2 = withUnsafeMutablePointer(to: &ca) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in accept(cfd, sp, &cl) } }
            if cfd2 < 0 {
                if errno == EAGAIN || errno == EWOULDBLOCK { break }
                self.log("HARPOON_BALLOON_CONTROL_ACCEPT_FAILED \(String(cString:strerror(errno)))")
                break
            }
            self.log("HARPOON_BALLOON_CONTROL_ACCEPT fd=\(cfd2)")
            let fl = fcntl(cfd2, F_GETFL, 0)
            _ = fcntl(cfd2, F_SETFL, fl | O_NONBLOCK)
            self.balloonBuffers[cfd2] = Data()
            let cs = DispatchSource.makeReadSource(fileDescriptor: cfd2, queue: .main)
            self.balloonClients[cfd2] = cs
            cs.setEventHandler { [weak self] in
                guard let self = self else { return }
                var buf = [UInt8](repeating: 0, count: 8192)
                let n = read(cfd2, &buf, buf.count)
                if n > 0 {
                    self.log("HARPOON_BALLOON_CONTROL_READ fd=\(cfd2) bytes=\(n)")
                    var data = self.balloonBuffers[cfd2] ?? Data()
                    data.append(contentsOf: buf[0..<n])
                    self.balloonBuffers[cfd2] = data
                    while let nl = data.range(of: Data([UInt8(ascii: "\n")])) {
                        let lineData = data.subdata(in: 0..<nl.lowerBound)
                        let line = String(data: lineData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                        data.removeSubrange(0..<nl.upperBound)
                        self.balloonBuffers[cfd2] = data
                        if line.isEmpty { continue }
                        self.handleBalloonLine(line, balloon: balloon)
                    }
                    self.balloonBuffers[cfd2] = data
                } else if n == 0 {
                    self.log("HARPOON_BALLOON_CONTROL_EOF fd=\(cfd2)")
                    if let data = self.balloonBuffers[cfd2], !data.isEmpty {
                        let line = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                        if !line.isEmpty { self.handleBalloonLine(line, balloon: balloon) }
                    }
                    cs.cancel(); self.balloonClients.removeValue(forKey: cfd2); self.balloonBuffers.removeValue(forKey: cfd2); close(cfd2)
                } else {
                    if errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR {
                        self.log("HARPOON_BALLOON_CONTROL_READ_FAILED fd=\(cfd2) err=\(String(cString:strerror(errno)))")
                        cs.cancel(); self.balloonClients.removeValue(forKey: cfd2); self.balloonBuffers.removeValue(forKey: cfd2); close(cfd2)
                    }
                }
            }
            cs.resume()
        }
    }
    source.setCancelHandler { close(cfd); try? FileManager.default.removeItem(atPath: self.config.balloonControlPath) }
    source.resume()
}

func handleBalloonLine(_ line: String, balloon: VZVirtioTraditionalMemoryBalloonDevice) {
    log("HARPOON_BALLOON_CONTROL_LINE \(line)")
    var token = line.lowercased()
    if token.hasPrefix("balloon") { token = String(token.dropFirst(7)).trimmingCharacters(in: .whitespaces) }
    else if token.hasPrefix("target") { token = String(token.dropFirst(6)).trimmingCharacters(in: .whitespaces) }
    token = token.replacingOccurrences(of: "mib", with: "").replacingOccurrences(of: "mb", with: "").replacingOccurrences(of: "m", with: "").trimmingCharacters(in: .whitespaces)
    var requested: UInt64?
    if let v = UInt64(token) {
        if v < 8192 { requested = v * 1024 * 1024 } else { requested = v }
    }
    if let req = requested {
        log("HARPOON_BALLOON_TARGET_REQUEST \(req)")
        let configuredBytes = config.memorySizeBytes
        let floorBytes: UInt64 = 512 * 1024 * 1024
        var rejectReason: String? = nil
        if req > configuredBytes {
            rejectReason = "exceeds configured memory \(configuredBytes) (\(configuredBytes/1024/1024) MiB)"
        } else if req < floorBytes {
            rejectReason = "below floor 512 MiB"
        } else if req % (1024 * 1024) != 0 {
            rejectReason = "must be a whole MiB"
        }
        if let reason = rejectReason {
            log("HARPOON_BALLOON_TARGET_REJECT requested=\(req) reason=\(reason)")
        } else {
            balloon.targetVirtualMachineMemorySize = req
            let set = balloon.targetVirtualMachineMemorySize
            log("HARPOON_BALLOON_TARGET_SET \(set)")
            log("HARPOON_BALLOON_TARGET_APPLIED requested=\(req) actual=\(set) MiB=\(set/1024/1024)")
        }
    } else {
        log("HARPOON_BALLOON_TARGET_PARSE_FAILED \(line)")
    }
}

}
