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
    // management channel (Stage 3A) — vsock 2377, unix 0600, no TCP
    var mgmtListenerFd: Int32 = -1
    var mgmtListenerSource: DispatchSourceRead?
    var ownsMgmtSocket = false

    init(config: RuntimeConfig, vsockDevice: VZVirtioSocketDevice?, balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?, log: @escaping (String)->Void) {
        self.config = config
        self.vsockDevice = vsockDevice
        self.balloonDevice = balloonDevice
        self.log = log
    }

    func startAll() {
        startUnixBridge()
        startBalloonControl()
        startMgmtBridge()
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
        if mgmtListenerSource != nil {
            mgmtListenerSource?.cancel(); mgmtListenerSource = nil
            mgmtListenerFd = -1
        } else if mgmtListenerFd >= 0 {
            close(mgmtListenerFd); mgmtListenerFd = -1
        }
        if ownsMgmtSocket {
            try? FileManager.default.removeItem(atPath: config.mgmtSocketPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.mgmtSocketPath) (owned) end")
            ownsMgmtSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.mgmtSocketPath) (not owned) end")
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

    // MARK: - Management channel (Stage 3A) vsock 2377, unix 0600, no TCP, no SSH
    func startMgmtBridge() {
        guard vsockDevice != nil else { log("HARPOON_MGMT_BRIDGE_SKIP no vsock"); return }
        if FileManager.default.fileExists(atPath: config.mgmtSocketPath) {
            if isSocketLive(config.mgmtSocketPath) {
                log("HARPOON_ALREADY_RUNNING mgmt \(config.mgmtSocketPath) in use")
                return
            }
            log("HARPOON_STALE_CLEANUP removing stale \(config.mgmtSocketPath)")
            try? FileManager.default.removeItem(atPath: config.mgmtSocketPath)
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { log("HARPOON_MGMT_FAILED socket \(String(cString:strerror(errno)))"); return }
        mgmtListenerFd = fd
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
        _ = config.mgmtSocketPath.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
        let len = socklen_t(MemoryLayout<sockaddr_un>.size)
        let br = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in Darwin.bind(fd, sp, len) } }
        guard br == 0 else {
            if errno == EADDRINUSE { log("HARPOON_ALREADY_RUNNING mgmt bind \(config.mgmtSocketPath) \(String(cString:strerror(errno)))") } else { log("HARPOON_MGMT_FAILED bind \(String(cString:strerror(errno)))") }
            close(fd); mgmtListenerFd = -1; return
        }
        chmod(config.mgmtSocketPath, 0o600)
        guard listen(fd, 16) == 0 else { log("HARPOON_MGMT_FAILED listen \(String(cString:strerror(errno)))"); close(fd); mgmtListenerFd = -1; return }
        ownsMgmtSocket = true
        log("HARPOON_MGMT_LISTENING \(config.mgmtSocketPath) 0600 -> vsock:\(config.mgmtVsockPort)")
        let flags = fcntl(fd, F_GETFL, 0)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
        var nextId = 0
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        mgmtListenerSource = source
        source.setEventHandler { [weak self] in
            guard let self = self else { return }
            while true {
                var ca = sockaddr_un()
                var cl: socklen_t = socklen_t(MemoryLayout<sockaddr_un>.size)
                let cfd = withUnsafeMutablePointer(to: &ca) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in accept(fd, sp, &cl) } }
                if cfd < 0 {
                    if errno == EAGAIN || errno == EWOULDBLOCK { break }
                    self.log("HARPOON_MGMT_ACCEPT_FAILED \(String(cString:strerror(errno)))")
                    break
                }
                let bid = nextId; nextId += 1
                self.log("HARPOON_MGMT_ACCEPT \(bid) fd=\(cfd)")
                guard let dev = self.vsockDevice else { self.log("HARPOON_MGMT_CLOSE \(bid) vsock not ready"); close(cfd); continue }
                dev.connect(toPort: self.config.mgmtVsockPort) { result in
                    switch result {
                    case .failure(let e):
                        self.log("HARPOON_MGMT_VSOCK_CONNECT_FAILURE \(bid) port \(self.config.mgmtVsockPort) error \(e)")
                        self.log("HARPOON_MGMT_CLOSE \(bid) vsock connect failed")
                        close(cfd)
                    case .success(let conn):
                        let vfd = conn.fileDescriptor
                        self.log("HARPOON_MGMT_VSOCK_CONNECTED \(bid) vsockFd=\(vfd) clientFd=\(cfd)")
                        // simple bidirectional pipe, vsock<->unix
                        let cflags = fcntl(cfd, F_GETFL, 0); if cflags >= 0 { _ = fcntl(cfd, F_SETFL, cflags | O_NONBLOCK) }
                        let vflags = fcntl(vfd, F_GETFL, 0); if vflags >= 0 { _ = fcntl(vfd, F_SETFL, vflags | O_NONBLOCK) }
                        let cr = DispatchSource.makeReadSource(fileDescriptor: cfd, queue: .global())
                        let vr = DispatchSource.makeReadSource(fileDescriptor: vfd, queue: .global())
                        var closedCr = false, closedVr = false, closed = false
                        func closeBoth(_ reason: String) {
                            if closed { return }; closed = true
                            cr.cancel(); vr.cancel(); close(cfd); conn.close()
                            self.log("HARPOON_MGMT_CLOSE \(bid) \(reason) clientFd=\(cfd) vsockFd=\(vfd)")
                        }
                        cr.setEventHandler {
                            var buf = [UInt8](repeating: 0, count: 8192)
                            let n = buf.withUnsafeMutableBytes { ptr in read(cfd, ptr.baseAddress!, ptr.count) }
                            if n == 0 { closedCr = true; cr.cancel(); shutdown(vfd, SHUT_WR); self.log("HARPOON_MGMT_CLIENT_EOF \(bid)"); if closedVr { closeBoth("both EOF") }; return }
                            if n < 0 { if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }; self.log("HARPOON_MGMT_CLOSE \(bid) client read \(String(cString:strerror(errno)))"); closeBoth("client read"); return }
                            var off = 0
                            while off < n {
                                let w = buf.withUnsafeBytes { ptr in write(vfd, ptr.baseAddress!.advanced(by: off), n-off) }
                                if w > 0 { off += Int(w); continue }
                                if w < 0 && errno == EINTR { continue }
                                if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                if w < 0 && errno == EPIPE { closeBoth("vsock EPIPE"); return }
                                self.log("HARPOON_MGMT_CLOSE \(bid) vsock write \(String(cString:strerror(errno)))"); closeBoth("vsock write"); return
                            }
                        }
                        vr.setEventHandler {
                            var buf = [UInt8](repeating: 0, count: 8192)
                            let n = buf.withUnsafeMutableBytes { ptr in read(vfd, ptr.baseAddress!, ptr.count) }
                            if n == 0 { closedVr = true; vr.cancel(); shutdown(cfd, SHUT_WR); self.log("HARPOON_MGMT_VSOCK_EOF \(bid)"); if closedCr { closeBoth("both EOF") }; return }
                            if n < 0 { if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }; self.log("HARPOON_MGMT_CLOSE \(bid) vsock read \(String(cString:strerror(errno)))"); closeBoth("vsock read"); return }
                            var off = 0
                            while off < n {
                                let w = buf.withUnsafeBytes { ptr in write(cfd, ptr.baseAddress!.advanced(by: off), n-off) }
                                if w > 0 { off += Int(w); continue }
                                if w < 0 && errno == EINTR { continue }
                                if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                                if w < 0 && errno == EPIPE { closeBoth("client EPIPE"); return }
                                self.log("HARPOON_MGMT_CLOSE \(bid) client write \(String(cString:strerror(errno)))"); closeBoth("client write"); return
                            }
                        }
                        cr.resume(); vr.resume()
                    }
                }
            }
        }
        source.setCancelHandler { close(fd) }
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
