import Foundation
import Virtualization

// ponytail: bridges are explicit — each owns one FD + DispatchSource, cleaned centrally on STOPPING, no global state
final class BridgeSet {
    let config: RuntimeConfig
    let vsockDevice: VZVirtioSocketDevice?
    let balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?
    let log: (String)->Void
    lazy var translator: HostPathTranslator = {
        let roots = config.sharedRoots + [SharedRoot(hostPath: config.shareHostPath, guestPath: "/mnt/harpoon-share", tag: config.virtioFSTag)]
        return HostPathTranslator(roots: roots, log: log)
    }()

    // docker sock bridge
    var listenerFd: Int32 = -1
    var listenerSource: DispatchSourceRead?
    var ownsDockerSocket = false
    // M5 dynamic port publishing
    var portManager: PortForwardManager?
    var guestIPPoll: DispatchSourceTimer?
    var guestIP: String?
    // legacy single-forward stubs (kept to keep _legacy compiling, not used)
    var hostForwardFd: Int32 = -1
    var hostForwardSource: DispatchSourceRead?
    var hostForwardGuestIP: String?
    var hostForwardStarted = false
    // balloon control
    var balloonControlFd: Int32 = -1
    var balloonControlSource: DispatchSourceRead?
    var ownsBalloonControlSocket = false
    var balloonClients: [Int32: DispatchSourceRead] = [:]
    var balloonBuffers: [Int32: Data] = [:]

    init(config: RuntimeConfig, vsockDevice: VZVirtioSocketDevice?, balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?, log: @escaping (String)->Void) {
        self.config = config
        self.vsockDevice = vsockDevice
        self.balloonDevice = balloonDevice
        self.log = log
    }

    func startAll() {
        startUnixBridge()
        startBalloonControl()
        startPortForwarding()
    }

    func stopAll() {
        log("HARPOON_BRIDGES_STOP_ALL begin dockerSock=\(config.dockerSocketPath) balloonControl=\(config.balloonControlPath) listenerFd=\(listenerFd) balloonFd=\(balloonControlFd) ownsDocker=\(ownsDockerSocket) ownsBalloon=\(ownsBalloonControlSocket)")
        // FD ownership: DispatchSource cancelHandler owns close; stopAll only cancels and nils.
        if listenerSource != nil {
            listenerSource?.cancel(); listenerSource = nil
            listenerFd = -1
        } else if listenerFd >= 0 {
            // fallback if source missing but fd leaked
            close(listenerFd); listenerFd = -1
        }
        // socket pathname removed only if this BridgeSet owns it
        if ownsDockerSocket {
            try? FileManager.default.removeItem(atPath: config.dockerSocketPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.dockerSocketPath) (owned)")
            ownsDockerSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.dockerSocketPath) (not owned)")
        }
        portManager?.stopAll()
        portManager = nil
        guestIPPoll?.cancel(); guestIPPoll = nil
        guestIP = nil
        log("HOST_FORWARD_CLEANED")
        if balloonControlSource != nil {
            balloonControlSource?.cancel(); balloonControlSource = nil
            balloonControlFd = -1
        } else if balloonControlFd >= 0 {
            close(balloonControlFd); balloonControlFd = -1
        }
        for (fd, src) in balloonClients { src.cancel(); close(fd) }
        balloonClients.removeAll(); balloonBuffers.removeAll()
        if ownsBalloonControlSocket {
            try? FileManager.default.removeItem(atPath: config.balloonControlPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.balloonControlPath) (owned) end")
            ownsBalloonControlSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.balloonControlPath) (not owned) end")
        }
    }

    func isSocketLive(_ path: String) -> Bool {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        if fd < 0 { return false }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
        _ = path.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
        let len = socklen_t(MemoryLayout<sockaddr_un>.size)
        let ret = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(fd, sp, len) } }
        close(fd)
        return ret == 0
    }

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
                                    // helper: check for absolute host bind sources outside shared roots
                                    func firstUnsupportedHostPath(in data: Data) -> String? {
                                        guard let json = try? JSONSerialization.jsonObject(with: data, options: []) as? [String: Any] else { return nil }
                                        // HostConfig.Binds
                                        if let hostConfig = json["HostConfig"] as? [String: Any], let binds = hostConfig["Binds"] as? [String] {
                                            for b in binds {
                                                let parts = b.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                                                guard parts.count >= 2 else { continue }
                                                let src = parts[0]
                                                guard src.hasPrefix("/") else { continue }
                                                if self.translator.translateHostPath(src) == nil { return src }
                                            }
                                        }
                                        // HostConfig.Mounts (bind only)
                                        if let hostConfig = json["HostConfig"] as? [String: Any], let mounts = hostConfig["Mounts"] as? [[String: Any]] {
                                            for m in mounts {
                                                guard let src = m["Source"] as? String, src.hasPrefix("/") else { continue }
                                                let t = m["Type"] as? String
                                                // only bind mounts are host paths; volume/named mounts are not host paths
                                                if t == nil || t == "bind" {
                                                    if self.translator.translateHostPath(src) == nil { return src }
                                                }
                                            }
                                        }
                                        // top-level Mounts
                                        if let mounts = json["Mounts"] as? [[String: Any]] {
                                            for m in mounts {
                                                guard let src = m["Source"] as? String, src.hasPrefix("/") else { continue }
                                                let t = m["Type"] as? String
                                                if t == nil || t == "bind" {
                                                    if self.translator.translateHostPath(src) == nil { return src }
                                                }
                                            }
                                        }
                                        // top-level Binds (rare)
                                        if let binds = json["Binds"] as? [String] {
                                            for b in binds {
                                                let parts = b.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                                                guard parts.count >= 2 else { continue }
                                                let src = parts[0]
                                                guard src.hasPrefix("/") else { continue }
                                                if self.translator.translateHostPath(src) == nil { return src }
                                            }
                                        }
                                        return nil
                                    }
                                    let isCreate = method == "POST" && path.contains("containers/create")
                                    var outData: Data = requestData
                                    if isCreate && bodyLen > 0 && !isChunked {
                                        if let unsupported = firstUnsupportedHostPath(in: bodyData) {
                                            let msg = "Harpoon: host path \"\(unsupported)\" is not shared. Supported Harpoon shared roots are /Users and /tmp (/private/tmp). Host path must be under /Users or /tmp to be bind-mounted. Unsupported host path: \(unsupported)"
                                            let errObj: [String: Any] = ["message": msg]
                                            let errBody = (try? JSONSerialization.data(withJSONObject: errObj, options: [])) ?? Data("{\"message\":\"unsupported host path\"}".utf8)
                                            let errHeader = "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: \(errBody.count)\r\n\r\n"
                                            var errData = Data(errHeader.utf8)
                                            errData.append(errBody)
                                            self.log("HARPOON_TRANSLATION_REJECT \(bid) \(unsupported) not shared")
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

    // M4: HTTP-aware first-request translation for host bind mounts
    struct TranslateResult { let foundRequest: Bool; let translated: Bool }
    func translateFirstRequestIfNeeded(clientFd: Int32, vsockFd: Int32, bid: Int) -> TranslateResult {
        // Root cause fix: clientFd may be O_NONBLOCK (listener is non-blocking), so a plain read returns
        // EAGAIN immediately before Docker has written. Capture and temporarily clear O_NONBLOCK, then
        // apply bounded SO_RCVTIMEO; restore on every exit via defer/helper.
        let originalFlags = fcntl(clientFd, F_GETFL, 0)
        var temporaryFlags = originalFlags
        if originalFlags >= 0 {
            temporaryFlags = originalFlags & ~O_NONBLOCK
            _ = fcntl(clientFd, F_SETFL, temporaryFlags)
        }
        log("HARPOON_TRANSLATION_CLIENT_FLAGS original=\(originalFlags) temporary=\(temporaryFlags)")
        // set 2s recv timeout for initial HTTP read to avoid blocking forever on non-HTTP or idle
        var tv = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(clientFd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        // small helper to restore flags/timeout on every path
        func restore() {
            if originalFlags >= 0 { _ = fcntl(clientFd, F_SETFL, originalFlags) }
            var tvZero = timeval(tv_sec: 0, tv_usec: 0)
            setsockopt(clientFd, SOL_SOCKET, SO_RCVTIMEO, &tvZero, socklen_t(MemoryLayout<timeval>.size))
        }
        defer { restore() }
        var buffer = Data()
        var foundRequest = false
        var translated = false
        let start = Date()
        while Date().timeIntervalSince(start) < 2.5 {
            var tmp = [UInt8](repeating: 0, count: 8192)
            let n = tmp.withUnsafeMutableBytes { ptr in read(clientFd, ptr.baseAddress!, ptr.count) }
            if n > 0 {
                buffer.append(contentsOf: tmp[0..<n])
                // check if we have complete headers
                if let headerEnd = buffer.range(of: Data("\r\n\r\n".utf8)) {
                    // parse headers
                    guard let headerStr = String(data: buffer.subdata(in: 0..<headerEnd.upperBound), encoding: .utf8) else {
                        // binary, not HTTP — forward as-is and fallback
                        break
                    }
                    let lines = headerStr.components(separatedBy: "\r\n")
                    guard let requestLine = lines.first else { break }
                    log("HARPOON_TRANSLATION_REQUEST \(requestLine)")
                    let isCreate = requestLine.contains("containers/create") && requestLine.hasPrefix("POST")
                    var contentLength: Int? = nil
                    var isChunked = false
                    for l in lines {
                        let lower = l.lowercased()
                        if lower.hasPrefix("content-length:") {
                            let v = l.components(separatedBy: ":").last?.trimmingCharacters(in: .whitespaces) ?? ""
                            contentLength = Int(v)
                        }
                        if lower.contains("transfer-encoding: chunked") { isChunked = true }
                    }
                    let headerLen = headerEnd.upperBound
                    let bodyStart = headerLen
                    let availableBody = buffer.count - bodyStart
                    var bodyComplete = false
                    var bodyData: Data? = nil
                    if isChunked {
                        // for chunked, we don't attempt translation — just forward as-is when we have some body? fallback to transparent
                        // wait a bit more for body then break to forward
                        if availableBody > 0 || Date().timeIntervalSince(start) > 1.0 {
                            bodyComplete = true
                            bodyData = buffer.subdata(in: bodyStart..<buffer.count)
                        }
                    } else if let cl = contentLength {
                        if availableBody >= cl {
                            bodyComplete = true
                            bodyData = buffer.subdata(in: bodyStart..<(bodyStart+cl))
                            // there may be extra data beyond body (pipelined next request) — keep it as leftover
                        } else {
                            // need more body
                            continue
                        }
                    } else {
                        // no body (GET etc)
                        bodyComplete = true
                        bodyData = Data()
                    }
                    if bodyComplete {
                        foundRequest = true
                        var outData: Data
                        var newHeaderStr = headerStr
                        var willTranslate = false
                        if isCreate, let body = bodyData, body.count > 0, !isChunked {
                            if let translatedBody = translator.translateCreateBody(body) {
                                // update Content-Length
                                translated = true
                                willTranslate = true
                                let newLen = translatedBody.count
                                // replace Content-Length header
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
                                    // insert before blank line
                                    newLines.insert("Content-Length: \(newLen)", at: newLines.count - 1)
                                }
                                newHeaderStr = newLines.joined(separator: "\r\n")
                                outData = Data(newHeaderStr.utf8) + translatedBody
                                // if there was extra data beyond body (pipelined), append it
                                let extraStart = bodyStart + (contentLength ?? 0)
                                if buffer.count > extraStart {
                                    outData.append(buffer.subdata(in: extraStart..<buffer.count))
                                }
                            } else {
                                // no translation needed, forward original
                                outData = buffer
                            }
                        } else {
                            outData = buffer
                        }
                        if willTranslate {
                            log("HARPOON_TRANSLATION_APPLIED \(bid)")
                        } else {
                            log("HARPOON_TRANSLATION_NO_CHANGE \(bid)")
                        }
                        // write transformed or original request to vsock
                        var off = 0
                        let total = outData.count
                        while off < total {
                            let w = outData.withUnsafeBytes { ptr in write(vsockFd, ptr.baseAddress!.advanced(by: off), total - off) }
                            if w > 0 { off += Int(w); continue }
                            if w < 0 && errno == EINTR { continue }
                            if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                            self.log("BRIDGE_TRANSLATION_WRITE_FAIL \(bid) \(String(cString:strerror(errno)))")
                            break
                        }
                        return TranslateResult(foundRequest: true, translated: translated)
                    }
                }
                // if buffer grows large without completing headers, fallback
                if buffer.count > 128*1024 {
                    break
                }
            } else if n == 0 {
                // client EOF before request complete
                log("HARPOON_TRANSLATION_FALLBACK client EOF before request")
                break
            } else {
                if errno == EAGAIN || errno == EWOULDBLOCK {
                    // timeout or no data yet
                    if buffer.isEmpty {
                        // no data within 2s, fallback
                        log("HARPOON_TRANSLATION_FALLBACK no HTTP request buffered")
                        break
                    }
                    // check if we have headers but need more body, continue loop
                    // if timeout and we have partial, forward as-is
                    if Date().timeIntervalSince(start) > 2.0 {
                        break
                    }
                    usleep(10000)
                    continue
                }
                if errno == EINTR { continue }
                // other error
                break
            }
        }
        // fallback: if we buffered some data but didn't complete HTTP, forward it as-is
        if !buffer.isEmpty {
            var off = 0
            while off < buffer.count {
                let w = buffer.withUnsafeBytes { ptr in write(vsockFd, ptr.baseAddress!.advanced(by: off), buffer.count - off) }
                if w > 0 { off += Int(w); continue }
                if w < 0 && errno == EINTR { continue }
                if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                break
            }
            foundRequest = false
        }
        if !foundRequest {
            let reason = buffer.isEmpty ? "no HTTP request buffered" : "incomplete HTTP request fallback"
            log("HARPOON_TRANSLATION_FALLBACK \(reason)")
        }
        return TranslateResult(foundRequest: foundRequest, translated: translated)
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
            let allowedTiers: [UInt64] = [512, 768, 1024].map { $0 * 1024 * 1024 }
            let reqMiB = req / 1024 / 1024
            var rejectReason: String? = nil
            if req > configuredBytes {
                rejectReason = "exceeds configured memory \(configuredBytes) (\(configuredBytes/1024/1024) MiB)"
            } else if req < floorBytes {
                rejectReason = "below floor 512 MiB"
            } else if !allowedTiers.contains(req) {
                // also allow only tier values that are <= configured
                if ![512, 768, 1024].contains(Int(reqMiB)) {
                    rejectReason = "unsupported tier \(reqMiB) MiB (allowed 512/768/1024 <= configured \(config.memoryMIB))"
                } else if reqMiB > UInt64(config.memoryMIB) {
                    rejectReason = "exceeds configured \(config.memoryMIB) MiB"
                }
            }
            // For configured 512, only 512 allowed; for 768, 512/768; for 1024, all — covered by above
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

    // MARK: - Port forward (M5 dynamic)

    func startPortForwarding() {
        let mgr = PortForwardManager(log: log)
        mgr.setVsockDevice(vsockDevice)
        self.portManager = mgr
        mgr.startPolling()
        // guest IP discovery — poll serial log for HARPOON_GUEST_IP, fallback to 192.168.64.3
        let poll = DispatchSource.makeTimerSource(queue: .main)
        self.guestIPPoll = poll
        poll.schedule(deadline: .now()+1, repeating: 1)
        var attempts = 0
        poll.setEventHandler { [weak self] in
            guard let self = self else { poll.cancel(); return }
            attempts += 1
            if let ip = self.parseGuestIP() {
                poll.cancel()
                self.guestIP = ip
                self.log("HARPOON_GUEST_IP_DISCOVERED \(ip)")
                mgr.setGuestIP(ip)
                // trigger initial sync after DOCKER_READY
                mgr.scheduleSync(delayMs: 1000)
                mgr.scheduleSync(delayMs: 3000)
            } else if attempts > 15 {
                self.log("HOST_FORWARD_DISCOVERY_FAILED no HARPOON_GUEST_IP after 15s")
                poll.cancel()
                let fallback = "192.168.64.3"
                self.log("HOST_FORWARD_TRY_FALLBACK \(fallback)")
                self.guestIP = fallback
                mgr.setGuestIP(fallback)
                mgr.scheduleSync(delayMs: 1000)
            }
        }
        poll.resume()
        // also update vsock device if it becomes available later (already set)
    }

    func parseGuestIP() -> String? {
        guard let d = try? Data(contentsOf: URL(fileURLWithPath: config.serialLogPath)), let s = String(data: d, encoding: .utf8) else { return nil }
        var ip: String?
        for line in s.components(separatedBy: "\n") where line.contains("HARPOON_GUEST_IP") {
            let parts = line.components(separatedBy: "HARPOON_GUEST_IP")
            if let last = parts.last {
                let cand = last.trimmingCharacters(in: .whitespacesAndNewlines).components(separatedBy: " ").first ?? ""
                if cand.hasPrefix("192.") || cand.hasPrefix("10.") { ip = cand }
                else if cand.range(of: #"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$"#, options: .regularExpression) != nil { ip = cand }
            }
        }
        return ip
    }

    func _legacy_startHostPortForward(guestIP: String) { log("legacy hardcoded 8080 forward disabled"); return
    // legacy body retained below but unreachable — kept for reference, not used
    }
}
