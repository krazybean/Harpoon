import Foundation
import Virtualization

extension BridgeSet {
// MARK: - Unix bridge

func startUnixBridge() {
    if FileManager.default.fileExists(atPath: config.dockerSocketPath) {
        if isSocketLive(config.dockerSocketPath) {
            log("HARPOON_ALREADY_RUNNING dockerSocket \(config.dockerSocketPath) in use")
            return
        }
        log("HARPOON_STALE_CLEANUP removing stale \(config.dockerSocketPath)")
        try? FileManager.default.removeItem(atPath: config.dockerSocketPath)
    }
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { log("socket failed \(String(cString:strerror(errno)))"); return }
    listenerFd = fd
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
    _ = config.dockerSocketPath.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let br = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in Darwin.bind(fd, sp, len) } }
    guard br == 0 else {
        if errno == EADDRINUSE {
            log("HARPOON_ALREADY_RUNNING bind failed \(config.dockerSocketPath) \(String(cString:strerror(errno)))")
        } else {
            log("bind failed \(String(cString:strerror(errno)))")
        }
        close(fd); listenerFd = -1; return
    }
    chmod(config.dockerSocketPath, 0o600)
    guard listen(fd, 16) == 0 else { log("listen failed \(String(cString:strerror(errno)))"); close(fd); listenerFd = -1; return }
    ownsDockerSocket = true
    log("UNIX socket listening at \(config.dockerSocketPath) 0600")
    let flags = fcntl(fd, F_GETFL, 0)
    _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
    var nextId = 0
    let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
    listenerSource = source
    source.setEventHandler { [weak self] in
        guard let self = self else { return }
        while true {
            var ca = sockaddr_un()
            var cl: socklen_t = socklen_t(MemoryLayout<sockaddr_un>.size)
            let cfd = withUnsafeMutablePointer(to: &ca) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in accept(fd, sp, &cl) } }
            if cfd < 0 {
                if errno == EAGAIN || errno == EWOULDBLOCK { break }
                self.log("accept error \(String(cString:strerror(errno)))")
                break
            }
            let bid = nextId; nextId += 1
            self.log("BRIDGE_ACCEPT \(bid) fd=\(cfd)")
            guard let dev = self.vsockDevice else { self.log("BRIDGE_CLOSE \(bid) vsock not ready"); close(cfd); continue }
            dev.connect(toPort: self.config.vsockPort) { result in
                switch result {
                case .failure(let e):
                    self.log("VSOCK_CONNECT_FAILURE port \(self.config.vsockPort) error \(e)")
                    self.log("BRIDGE_CLOSE \(bid) vsock connect failed")
                    close(cfd)
                case .success(let conn):
                    let vfd = conn.fileDescriptor
                    self.log("BRIDGE_VSOCK_CONNECTED \(bid) vsockFd=\(vfd) clientFd=\(cfd)")
                    // M4: request-aware host-path translation for bind mounts, otherwise transparent proxy
                    // Use blocking read for first request to allow transformation, then fallback to transparent DispatchSource proxy
                    DispatchQueue.global().async { [weak self] in
                        guard let self = self else { close(cfd); conn.close(); return }
                        // M4 keep-alive streaming HTTP/1.1 parser for host-path translation.
                        // For each complete HTTP request on a keep-alive connection, parse request line + headers,
                        // honor Content-Length, translate POST .../containers/create via HostPathTranslator, otherwise forward verbatim.
                        // Detect Connection: Upgrade / Upgrade: tcp hijack and switch to transparent raw proxy.
                        // vsock->client remains transparent half-close aware; client->vsock is streaming parsed.
                        // Ensure clientFd is non-blocking for DispatchSource.
                        let cflags = fcntl(cfd, F_GETFL, 0)
                        if cflags >= 0 { _ = fcntl(cfd, F_SETFL, cflags | O_NONBLOCK) }
                        let vflags = fcntl(vfd, F_GETFL, 0)
                        if vflags >= 0 { _ = fcntl(vfd, F_SETFL, vflags | O_NONBLOCK) }
                        let cr = DispatchSource.makeReadSource(fileDescriptor: cfd, queue: .global())
                        let vr = DispatchSource.makeReadSource(fileDescriptor: vfd, queue: .global())
                        var closedCr = false, closedVr = false, closed = false
                        var hijacked = false
                        var clientBuf = Data()
                        func closeBoth(_ reason: String) {
                            if closed { return }; closed = true
                            cr.cancel(); vr.cancel(); close(cfd); conn.close()
                            self.log("BRIDGE_CLOSE \(bid) \(reason) clientFd=\(cfd) vsockFd=\(vfd)")
                        }
                        func writeAllToVsock(_ data: Data) -> Bool {
                            var off = 0
                            let total = data.count
                            while off < total {
                                let w = data.withUnsafeBytes { ptr in write(vfd, ptr.baseAddress!.advanced(by: off), total - off) }
                                if w > 0 { off += Int(w); continue }
                                if w < 0 && errno == EINTR { continue }
                                if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                if w < 0 && errno == EPIPE { closeBoth("vsock EPIPE during forward"); return false }
                                self.log("BRIDGE_CLOSE \(bid) vsock write error \(String(cString:strerror(errno)))"); closeBoth("vsock write"); return false
                            }
                            return true
                        }
                        // Helper to try to parse and forward as many complete requests as available in clientBuf.
                        // Returns true if buffer was consumed and should continue, false if need more data.
                        func drainClientBuffer() {
                            while true {
                                guard let headerEnd = clientBuf.range(of: Data("\r\n\r\n".utf8)) else { break }
                                guard let headerStr = String(data: clientBuf.subdata(in: 0..<headerEnd.upperBound), encoding: .utf8) else {
                                    // binary without valid UTF8 headers — treat as raw and hijack to transparent
                                    self.log("HARPOON_HTTP_HIJACK \(bid) switching-transparent")
                                    hijacked = true
                                    // flush entire buffer raw
                                    _ = writeAllToVsock(clientBuf)
                                    clientBuf.removeAll()
                                    break
                                }
                                let lines = headerStr.components(separatedBy: "\r\n")
                                guard let requestLine = lines.first, !requestLine.isEmpty else { break }
                                let parts = requestLine.split(separator: " ")
                                let method = parts.count > 0 ? String(parts[0]) : ""
                                let path = parts.count > 1 ? String(parts[1]) : ""
                                self.log("HARPOON_HTTP_REQUEST \(bid) \(method) \(path)")
                                // M5: trigger port sync on container lifecycle API
                                if path.contains("/containers/") && (method == "POST" || method == "DELETE") {
                                    if path.contains("/start") || path.contains("/stop") || path.contains("/restart") || path.contains("/create") || path.contains("/remove") || method == "DELETE" {
                                        self.portManager?.scheduleSync(delayMs: 800)
                                        self.portManager?.scheduleSync(delayMs: 2000)
                                    }
                                }
                                if method == "POST" && path.contains("/containers/create") {
                                    self.portManager?.scheduleSync(delayMs: 1000)
                                }
                                var contentLength: Int? = nil
                                var isChunked = false
                                var isUpgrade = false
                                for l in lines {
                                    let lower = l.lowercased()
                                    if lower.hasPrefix("content-length:") {
                                        let v = l.components(separatedBy: ":").last?.trimmingCharacters(in: .whitespaces) ?? ""
                                        contentLength = Int(v)
                                    }
                                    if lower.contains("transfer-encoding:") && lower.contains("chunked") { isChunked = true }
                                    if lower.hasPrefix("connection:") && lower.contains("upgrade") { isUpgrade = true }
                                    if lower.hasPrefix("upgrade:") && (lower.contains("tcp") || lower.contains("h2c") || lower.contains("hijack")) { isUpgrade = true }
                                }
                                // also catch general upgrade header without tcp keyword
                                let lowerHeader = headerStr.lowercased()
                                if lowerHeader.contains("connection: upgrade") && lowerHeader.contains("upgrade:") { isUpgrade = true }
                                let headerLen = headerEnd.upperBound
                                var bodyLen = 0
                                var needMore = false
                                if isChunked {
                                    // chunked body framing: need to find terminating 0 chunk. Until then wait for more data unless we decide to treat as hijack.
                                    // For containers/create chunked is not expected; still need to frame request boundary.
                                    // Look for terminating sequence \r\n0\r\n\r\n
                                    if let term = clientBuf.range(of: Data("\r\n0\r\n\r\n".utf8), options: [], in: headerLen..<clientBuf.count) {
                                        bodyLen = term.upperBound - headerLen
                                    } else if let term2 = clientBuf.range(of: Data("\n0\n\n".utf8)) {
                                        bodyLen = term2.upperBound - headerLen
                                    } else {
                                        // not yet complete chunked body
                                        if clientBuf.count > 1024*1024 { // avoid unbounded growth, fallback to raw
                                            self.log("HARPOON_HTTP_HIJACK \(bid) switching-transparent")
                                            hijacked = true
                                            _ = writeAllToVsock(clientBuf)
                                            clientBuf.removeAll()
                                            break
                                        }
                                        needMore = true
                                    }
                                } else if let cl = contentLength {
                                    bodyLen = cl
                                    if clientBuf.count < headerLen + bodyLen { needMore = true }
                                } else {
                                    bodyLen = 0
                                }
                                if needMore { break }
                                let totalLen = headerLen + bodyLen
                                if clientBuf.count < totalLen { break }
                                let requestData = clientBuf.subdata(in: 0..<totalLen)
                                let bodyData = bodyLen > 0 ? clientBuf.subdata(in: headerLen..<totalLen) : Data()
                                // hijack detection: after parsing complete request, if upgrade, forward and switch
                                if isUpgrade {
                                    self.log("HARPOON_HTTP_HIJACK \(bid) switching-transparent")
                                    _ = writeAllToVsock(requestData)
                                    clientBuf.removeSubrange(0..<totalLen)
                                    hijacked = true
                                    // flush any remaining buffered data raw (pipelined after hijack is raw stream)
                                    if !clientBuf.isEmpty {
                                        _ = writeAllToVsock(clientBuf)
                                        clientBuf.removeAll()
                                    }
                                    break
                                }
                                // Reject missing sources before Docker can create a directory for a file bind.
                                func firstInvalidHostPath(in data: Data) -> (path: String, reason: String)? {
                                    func issue(_ src: String) -> (path: String, reason: String)? {
                                        guard src.hasPrefix("/") else { return nil }
                                        guard self.translator.translateHostPath(src) != nil else { return (src, "not shared") }
                                        return FileManager.default.fileExists(atPath: src) ? nil : (src, "missing")
                                    }
                                    guard let json = try? JSONSerialization.jsonObject(with: data, options: []) as? [String: Any] else { return nil }
                                    // HostConfig.Binds
                                    if let hostConfig = json["HostConfig"] as? [String: Any], let binds = hostConfig["Binds"] as? [String] {
                                        for b in binds {
                                            let parts = b.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                                            guard parts.count >= 2 else { continue }
                                            let src = parts[0]
                                            if let found = issue(src) { return found }
                                        }
                                    }
                                    // HostConfig.Mounts (bind only)
                                    if let hostConfig = json["HostConfig"] as? [String: Any], let mounts = hostConfig["Mounts"] as? [[String: Any]] {
                                        for m in mounts {
                                            guard let src = m["Source"] as? String else { continue }
                                            let t = m["Type"] as? String
                                            // only bind mounts are host paths; volume/named mounts are not host paths
                                            if t == nil || t == "bind" {
                                                if let found = issue(src) { return found }
                                            }
                                        }
                                    }
                                    // top-level Mounts
                                    if let mounts = json["Mounts"] as? [[String: Any]] {
                                        for m in mounts {
                                            guard let src = m["Source"] as? String else { continue }
                                            let t = m["Type"] as? String
                                            if t == nil || t == "bind" {
                                                if let found = issue(src) { return found }
                                            }
                                        }
                                    }
                                    // top-level Binds (rare)
                                    if let binds = json["Binds"] as? [String] {
                                        for b in binds {
                                            let parts = b.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                                            guard parts.count >= 2 else { continue }
                                            let src = parts[0]
                                            if let found = issue(src) { return found }
                                        }
                                    }
                                    return nil
                                }
                                let isCreate = method == "POST" && path.contains("containers/create")
                                var outData: Data = requestData
                                if isCreate && bodyLen > 0 && !isChunked {
                                    if let invalid = firstInvalidHostPath(in: bodyData) {
                                        let msg = invalid.reason == "missing" ? "Harpoon: host bind source \"\(invalid.path)\" does not exist; refusing to let Docker create a directory with the wrong type." : "Harpoon: host path \"\(invalid.path)\" is not shared. Supported Harpoon shared roots are /Users and /tmp (/private/tmp). Host path must be under /Users or /tmp to be bind-mounted. Unsupported host path: \(invalid.path)"
                                        let errObj: [String: Any] = ["message": msg]
                                        let errBody = (try? JSONSerialization.data(withJSONObject: errObj, options: [])) ?? Data("{\"message\":\"unsupported host path\"}".utf8)
                                        let errHeader = "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: \(errBody.count)\r\n\r\n"
                                        var errData = Data(errHeader.utf8)
                                        errData.append(errBody)
                                        self.log("HARPOON_TRANSLATION_REJECT \(bid) path=\(invalid.path) reason=\(invalid.reason)")
                                        // write error directly to client (Docker CLI)
                                        var off = 0
                                        while off < errData.count {
                                            let w = errData.withUnsafeBytes { ptr in write(cfd, ptr.baseAddress!.advanced(by: off), errData.count - off) }
                                            if w > 0 { off += Int(w); continue }
                                            if w < 0 && errno == EINTR { continue }
                                            if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                            break
                                        }
                                        clientBuf.removeSubrange(0..<totalLen)
                                        // do not forward to vsock; continue to next pipelined request
                                        continue
                                    }
                                    if let translatedBody = self.translator.translateCreateBody(bodyData) {
                                        // rebuild with updated Content-Length
                                        let newLen = translatedBody.count
                                        var newLines: [String] = []
                                        var replaced = false
                                        for l in lines {
                                            if l.lowercased().hasPrefix("content-length:") {
                                                newLines.append("Content-Length: \(newLen)")
                                                replaced = true
                                            } else {
                                                newLines.append(l)
                                            }
                                        }
                                        if !replaced && newLen > 0 {
                                            newLines.insert("Content-Length: \(newLen)", at: newLines.count - 1)
                                        }
                                        let newHeaderStr = newLines.joined(separator: "\r\n")
                                        outData = Data(newHeaderStr.utf8) + translatedBody
                                        // preserve any pipelined extra already in clientBuf beyond this request? already handled via totalLen
                                        self.log("HARPOON_TRANSLATION_APPLIED \(bid) \(path)")
                                    }
                                }
                                if !writeAllToVsock(outData) { clientBuf.removeAll(); break }
                                clientBuf.removeSubrange(0..<totalLen)
                                // continue to parse next pipelined request if any
                            }
                        }
                        cr.setEventHandler {
                            var buf = [UInt8](repeating: 0, count: 8192)
                            let n = buf.withUnsafeMutableBytes { ptr in read(cfd, ptr.baseAddress!, ptr.count) }
                            if n == 0 { closedCr = true; cr.cancel(); shutdown(vfd, SHUT_WR); self.log("BRIDGE_CLIENT_EOF \(bid)"); if closedVr { closeBoth("both EOF") }; return }
                            if n < 0 {
                                if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
                                self.log("BRIDGE_CLOSE \(bid) client read error \(String(cString:strerror(errno)))"); closeBoth("client read"); return
                            }
                            if hijacked {
                                // transparent raw forwarding after hijack
                                var off = 0
                                while off < n {
                                    let w = buf.withUnsafeBytes { ptr in write(vfd, ptr.baseAddress!.advanced(by: off), n-off) }
                                    if w > 0 { off += Int(w); continue }
                                    if w < 0 && errno == EINTR { continue }
                                    if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                    if w < 0 && errno == EPIPE { closeBoth("vsock EPIPE"); return }
                                    self.log("BRIDGE_CLOSE \(bid) vsock write error \(String(cString:strerror(errno)))"); closeBoth("vsock write"); return
                                }
                                return
                            }
                            clientBuf.append(contentsOf: buf[0..<n])
                            if clientBuf.count > 4*1024*1024 {
                                // safety: avoid unbounded buffer, switch to transparent
                                self.log("HARPOON_HTTP_HIJACK \(bid) switching-transparent")
                                hijacked = true
                                _ = writeAllToVsock(clientBuf)
                                clientBuf.removeAll()
                                return
                            }
                            drainClientBuffer()
                            // if buffer still holds incomplete request, wait for more data (do not forward partial)
                        }
                        vr.setEventHandler {
                            var buf = [UInt8](repeating: 0, count: 8192)
                            let n = buf.withUnsafeMutableBytes { ptr in read(vfd, ptr.baseAddress!, ptr.count) }
                            if n == 0 { closedVr = true; vr.cancel(); shutdown(cfd, SHUT_WR); self.log("BRIDGE_VSOCK_EOF \(bid)"); if closedCr { closeBoth("both EOF") }; return }
                            if n < 0 {
                                if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
                                self.log("BRIDGE_CLOSE \(bid) vsock read \(String(cString:strerror(errno)))"); closeBoth("vsock read"); return
                            }
                            var off = 0
                            while off < n {
                                let w = buf.withUnsafeBytes { ptr in write(cfd, ptr.baseAddress!.advanced(by: off), n-off) }
                                if w > 0 { off += Int(w); continue }
                                if w < 0 && errno == EINTR { continue }
                                if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                if w < 0 && errno == EPIPE { closeBoth("client EPIPE"); return }
                                self.log("BRIDGE_CLOSE \(bid) client write \(String(cString:strerror(errno)))"); closeBoth("client write"); return
                            }
                        }
                        cr.resume(); vr.resume()
                        self.log("BRIDGE_VSOCK_CONNECTED \(bid) proxy start streaming")
                    }
                }
            }
        }
    }
    source.setCancelHandler { close(fd) }
    source.resume()
}

}
