import Foundation
import Virtualization

// ponytail: M5 dynamic port publishing — N listeners, one per published Docker HostPort, loopback-only safety, FD ownership per listing
final class PortForwardManager {
    let log: (String) ->Void
    var guestIP: String?
    // key = "127.0.0.1:8080/tcp"  (hostAddr:hostPort/proto)
    private var forwards: [String: Forward] = [:]
    private var dockerPoll: DispatchSourceTimer?
    private var vsockDevice: VZVirtioSocketDevice?
    private var syncInFlight = false

    struct Forward {
        let hostAddr: String
        let hostPort: UInt16
        let guestHostPort: UInt16 // PublicPort — Harpoon forwards to guest HostPort, NOT container PrivatePort
        let containerPort: UInt16 // PrivatePort — metadata/observability
        let proto: String // tcp only for M5
        let containerId: String
        let containerName: String
        let guestIP: String
        let fd: Int32
        let source: DispatchSourceRead
    }

    init(log: @escaping (String) ->Void) {
        self.log = log
    }

    func setVsockDevice(_ dev: VZVirtioSocketDevice?) {
        self.vsockDevice = dev
    }

    func setGuestIP(_ ip: String) {
        if guestIP == ip { return }
        guestIP = ip
        log("HARPOON_GUEST_IP_DISCOVERED \(ip) -> port manager guestIP set")
        // trigger sync
        scheduleSync(delayMs: 200)
    }

    func startPolling() {
        // M14: idle optimization — was 2s unconditional (30 wakeups/min), now 10s (6/min, 80% reduction)
        // Justification: published ports not latency-critical (dev tolerates 10s); sync also triggered via scheduleSync on guest IP/container changes, so 10s is fallback only.
        // Before: HARPOON_PORT_SYNC_START every 2s even idle (see harpoon.log). After: 10s reduces idle CPU wakeups while preserving correctness (reconciled within 10s).
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now()+5, repeating: 10)
        t.setEventHandler { [weak self] in self?.sync() }
        t.resume()
        dockerPoll = t
    }

    func stopAll() {
        dockerPoll?.cancel(); dockerPoll = nil
        for (_, f) in forwards {
            f.source.cancel() // close in cancelHandler
        }
        forwards.removeAll()
        log("HARPOON_PORT_FORWARD_CLEANED all")
    }

    func scheduleSync(delayMs: Int = 500) {
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(delayMs)) { [weak self] in self?.sync() }
    }

    // Docker container Ports from /containers/json
    struct DockerPort: Decodable {
        let IP: String?
        let PrivatePort: Int
        let PublicPort: Int?
        let `Type`: String
    }
    struct DockerContainerSummary: Decodable {
        let Id: String
        let Names: [String]
        let State: String
        let Ports: [DockerPort]?
    }

    func vsockFetch(path: String, completion: @escaping (Data?) ->Void) {
        // VZVirtioSocketDevice.connect(toPort:) must be called on VZVirtualMachine's queue (main)
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in self?.vsockFetch(path: path, completion: completion) }
            return
        }
        guard let dev = vsockDevice else { completion(nil); return }
        log("HARPOON_PORT_VSOCK_CONNECT \(path)")
        // MAIN: VZ connect — required on main
        dev.connect(toPort: 2375) { result in
            // VZ completion is on main
            switch result {
            case .failure(_):
                completion(nil)
            case .success(let conn):
                self.log("HARPOON_PORT_VSOCK_CONNECTED \(path)")
                let fd = conn.fileDescriptor
                // BACKGROUND: blocking fd I/O off main
                DispatchQueue.global().async {
                    let req = "GET \(path) HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    _ = req.withCString { cstr in write(fd, cstr, strlen(cstr)) }
                    var data = Data()
                    var buf = [UInt8](repeating: 0, count: 8192)
                    var tv = timeval(tv_sec: 3, tv_usec: 0)
                    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
                    while true {
                        let n = buf.withUnsafeMutableBytes { ptr in read(fd, ptr.baseAddress!, ptr.count) }
                        if n > 0 { data.append(contentsOf: buf[0..<n]); continue }
                        if n == 0 { break }
                        if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { break }
                        break
                    }
                    // MAIN: VZ cleanup + completion back on main, keep syncInFlight on main
                    DispatchQueue.main.async {
                        conn.close()
                        if let hdrEnd = data.range(of: Data("\r\n\r\n".utf8)) {
                            let body = data.subdata(in: hdrEnd.upperBound..<data.count)
                            completion(body)
                        } else {
                            completion(nil)
                        }
                    }
                }
            }
        }
    }

    func sync() {
        // main-queue-owned: guestIP, forwards, syncInFlight, dockerPoll, reconcile, VZ connect
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in self?.sync() }
            return
        }
        guard !syncInFlight else { return }
        guard let gip = guestIP else { return }
        guard vsockDevice != nil else { return }
        syncInFlight = true
        log("HARPOON_PORT_SYNC_START")
        vsockFetch(path: "/containers/json?all=1") { [weak self] data in
            guard let self = self else { return }
            // keep syncInFlight on main
            assert(Thread.isMainThread)
            defer { self.syncInFlight = false }
            guard let data = data else {
                self.log("HARPOON_PORT_SYNC_RESULT containers=0 mappings=0")
                return
            }
            let decoder = JSONDecoder()
            guard let arr = try? decoder.decode([DockerContainerSummary].self, from: data) else {
                self.log("HARPOON_PORT_SYNC_RESULT containers=0 mappings=0")
                return
            }
            var desired: [String: (hostPort: UInt16, guestHostPort: UInt16, containerPort: UInt16, proto: String, containerId: String, containerName: String)] = [:]
            for c in arr where c.State == "running" {
                let name = c.Names.first ?? c.Id
                let cleanName = name.hasPrefix("/") ? String(name.dropFirst()) : name
                guard let ports = c.Ports else { continue }
                for p in ports {
                    guard let pub = p.PublicPort, pub > 0 && pub <= 65535 else { continue }
                    guard p.`Type`.lowercased() == "tcp" else {
                        // UDP deferred
                        continue
                    }
                    let hostPort = UInt16(pub)
                    let guestHostPort = UInt16(pub) // PublicPort — Harpoon forwards to guest HostPort
                    let containerPort = UInt16(p.PrivatePort) // PrivatePort — metadata
                    let proto = p.`Type`.lowercased()
                    let dockerHostIp = p.IP ?? "0.0.0.0"
                    // safety: map unspecified to loopback
                    let hostAddr = self.hostAddrForDockerIP(dockerHostIp)
                    let key = "\(hostAddr):\(hostPort)/\(proto)"
                    desired[key] = (hostPort, guestHostPort, containerPort, proto, c.Id, cleanName)
                }
            }
            self.log("HARPOON_PORT_SYNC_RESULT containers=\(arr.count) mappings=\(desired.count)")
            self.reconcile(desired: desired, guestIP: gip)
        }
    }

    private func hostAddrForDockerIP(_ ip: String) -> String {
        let t = ip.trimmingCharacters(in: .whitespaces)
        if t.isEmpty || t == "0.0.0.0" || t == "::" || t == "0.0.0.0/0" { return "127.0.0.1" }
        if t == "127.0.0.1" { return "127.0.0.1" }
        // Phase1 safety: do not expose LAN
        return "127.0.0.1"
    }

    private func reconcile(desired: [String: (hostPort: UInt16, guestHostPort: UInt16, containerPort: UInt16, proto: String, containerId: String, containerName: String)], guestIP: String) {
        // remove no-longer-desired
        for (key, f) in forwards where desired[key] == nil {
            log("HARPOON_PORT_FORWARD_REMOVE \(f.containerName) \(f.hostAddr):\(f.hostPort) -> \(f.guestIP):\(f.guestHostPort)/\(f.proto) containerPort=\(f.containerPort)")
            f.source.cancel()
            forwards.removeValue(forKey: key)
        }
        // add new
        for (key, val) in desired where forwards[key] == nil {
            let shouldRestore = forwards.isEmpty && !desired.isEmpty // first sync after start = restore
            addForward(key: key, hostAddr: hostAddrForKey(key), hostPort: val.hostPort, guestHostPort: val.guestHostPort, containerPort: val.containerPort, proto: val.proto, containerId: val.containerId, containerName: val.containerName, guestIP: guestIP, isRestore: shouldRestore)
        }
    }

    private func hostAddrForKey(_ key: String) -> String {
        // key format "127.0.0.1:8080/tcp"
        if let colon = key.firstIndex(of: ":") {
            return String(key[..<colon])
        }
        return "127.0.0.1"
    }

    private func addForward(key: String, hostAddr: String, hostPort: UInt16, guestHostPort: UInt16, containerPort: UInt16, proto: String, containerId: String, containerName: String, guestIP: String, isRestore: Bool) {
        if proto != "tcp" {
            log("HARPOON_PORT_FORWARD_SKIP \(containerName) \(hostAddr):\(hostPort)/\(proto) udp deferred")
            return
        }
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { log("HARPOON_PORT_FORWARD_COLLISION \(containerName) \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort) socket failed \(String(cString:strerror(errno)))"); return }
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(hostPort.bigEndian)
        addr.sin_addr.s_addr = hostAddr == "127.0.0.1" ? inet_addr("127.0.0.1") : inet_addr((hostAddr as NSString).utf8String)
        // bind
        let br = withUnsafePointer(to: &addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in Darwin.bind(fd, sp, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
        if br != 0 {
            let err = String(cString:strerror(errno))
            log("HARPOON_PORT_FORWARD_COLLISION \(containerName) \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort) bind failed \(err)")
            close(fd)
            return
        }
        guard listen(fd, 16) == 0 else {
            log("HARPOON_PORT_FORWARD_COLLISION \(containerName) \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort) listen failed \(String(cString:strerror(errno)))")
            close(fd); return
        }
        let flags = fcntl(fd, F_GETFL, 0)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
        log("HARPOON_PORT_FORWARD_ADD \(containerName) \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort) id=\(String(containerId.prefix(12)))")
        log("HARPOON_PORT_FORWARD_LISTENING \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort) \(containerName)")
        if isRestore {
            log("HARPOON_PORT_FORWARD_RESTORE \(containerName) \(hostAddr):\(hostPort) -> \(guestIP):\(guestHostPort)/\(proto) containerPort=\(containerPort)")
        }
        var nextId = 0
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        // keep strong ref via forwards dict
        source.setEventHandler { [weak self] in
            guard let self = self else { return }
            while true {
                var ca = sockaddr_in()
                var cl = socklen_t(MemoryLayout<sockaddr_in>.size)
                let cfd = withUnsafeMutablePointer(to: &ca) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in accept(fd, sp, &cl) } }
                if cfd < 0 {
                    if errno == EAGAIN || errno == EWOULDBLOCK { break }
                    self.log("HOST_FORWARD_ACCEPT_ERROR \(String(cString:strerror(errno)))")
                    break
                }
                let fid = nextId; nextId += 1
                self.log("HOST_FORWARD_ACCEPT \(fid) clientFd=\(cfd) -> \(guestIP):\(guestHostPort) containerPort=\(containerPort)")
                DispatchQueue.global().async {
                    let gfd = socket(AF_INET, SOCK_STREAM, 0)
                    if gfd < 0 { self.log("HOST_FORWARD_GUEST_SOCKET_FAILED \(fid) \(String(cString:strerror(errno)))"); close(cfd); return }
                    var gaddr = sockaddr_in()
                    gaddr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
                    gaddr.sin_family = sa_family_t(AF_INET)
                    gaddr.sin_port = in_port_t(guestHostPort.bigEndian)
                    _ = guestIP.withCString { cstr in inet_pton(AF_INET, cstr, &gaddr.sin_addr) }
                    let conn = withUnsafePointer(to: &gaddr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(gfd, sp, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
                    if conn != 0 { self.log("HOST_FORWARD_CONNECT_FAILED \(fid) -> \(guestIP):\(guestHostPort) containerPort=\(containerPort) \(String(cString:strerror(errno)))"); close(cfd); close(gfd); return }
                    self.log("HOST_FORWARD_CONNECTED \(fid) clientFd=\(cfd) guestFd=\(gfd) -> \(guestIP):\(guestHostPort) containerPort=\(containerPort)")
                    // per-connection proxy
                    let c2g = DispatchSource.makeReadSource(fileDescriptor: cfd, queue: .global())
                    let g2c = DispatchSource.makeReadSource(fileDescriptor: gfd, queue: .global())
                    var closedCr = false, closedGr = false, closed = false
                    func closeBoth(_ reason: String) {
                        if closed { return }; closed = true
                        c2g.cancel(); g2c.cancel(); close(cfd); close(gfd)
                        self.log("HOST_FORWARD_CLOSE \(fid) \(reason)")
                    }
                    c2g.setEventHandler {
                        var buf = [UInt8](repeating: 0, count: 8192)
                        let n = buf.withUnsafeMutableBytes { ptr in read(cfd, ptr.baseAddress!, ptr.count) }
                        if n == 0 { closedCr = true; c2g.cancel(); shutdown(gfd, SHUT_WR); self.log("HOST_FORWARD_CLIENT_EOF \(fid)"); if closedGr { closeBoth("both EOF") }; return }
                        if n < 0 {
                            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
                            closeBoth("client read \(String(cString:strerror(errno)))"); return
                        }
                        var off = 0
                        while off < n {
                            let w = buf.withUnsafeBytes { ptr in write(gfd, ptr.baseAddress!.advanced(by: off), n-off) }
                            if w > 0 { off += Int(w); continue }
                            if w < 0 && errno == EINTR { continue }
                            if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                            if w < 0 && errno == EPIPE { closeBoth("guest EPIPE"); return }
                            closeBoth("guest write"); return
                        }
                    }
                    g2c.setEventHandler {
                        var buf = [UInt8](repeating: 0, count: 8192)
                        let n = buf.withUnsafeMutableBytes { ptr in read(gfd, ptr.baseAddress!, ptr.count) }
                        if n == 0 { closedGr = true; g2c.cancel(); shutdown(cfd, SHUT_WR); self.log("HOST_FORWARD_GUEST_EOF \(fid)"); if closedCr { closeBoth("both EOF") }; return }
                        if n < 0 {
                            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
                            closeBoth("guest read \(String(cString:strerror(errno)))"); return
                        }
                        var off = 0
                        while off < n {
                            let w = buf.withUnsafeBytes { ptr in write(cfd, ptr.baseAddress!.advanced(by: off), n-off) }
                            if w > 0 { off += Int(w); continue }
                            if w < 0 && errno == EINTR { continue }
                            if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                            if w < 0 && errno == EPIPE { closeBoth("client EPIPE"); return }
                            closeBoth("client write"); return
                        }
                    }
                    c2g.resume(); g2c.resume()
                }
            }
        }
        source.setCancelHandler { close(fd) }
        source.resume()
        let fwd = Forward(hostAddr: hostAddr, hostPort: hostPort, guestHostPort: guestHostPort, containerPort: containerPort, proto: proto, containerId: containerId, containerName: containerName, guestIP: guestIP, fd: fd, source: source)
        forwards[key] = fwd
    }
}
