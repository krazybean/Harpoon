import Foundation
import Darwin

// One accepted host client <-> one guest vsock stream, with a single owner for both descriptors. Foundation and Darwin only, so the
// lifecycle can be tested with socketpairs (harpoon/regression-bridge-pair-test.swift).
//
// What this replaces: each bridge kept its pump state in closure variables mutated by two concurrent handlers, closed descriptors
// while their dispatch sources were still registered, and after a client EOF waited for the guest to close with no bound, so a client
// that gave up (a status probe that timed out) left both descriptors open for as long as the guest stayed silent.

/// True when the other end of a unix-domain socket has been closed completely. A peer that only called shutdown(SHUT_WR) (a request
/// finished, still waiting for the answer) is NOT closed. poll() cannot tell the two apart on macOS (POLLHUP is set for both);
/// the write filter's EV_EOF is set only when the peer is gone.
func unixPeerClosed(_ fd: Int32) -> Bool {
    let kq = kqueue()
    guard kq >= 0 else { return false }
    defer { close(kq) }
    var change = kevent(ident: UInt(fd), filter: Int16(EVFILT_WRITE), flags: UInt16(EV_ADD | EV_ONESHOT), fflags: 0, data: 0, udata: nil)
    var out = kevent()
    var none = timespec(tv_sec: 0, tv_nsec: 0)
    return kevent(kq, &change, 1, &out, 1, &none) > 0 && (out.flags & UInt16(EV_EOF)) != 0
}

struct BridgeLimits {
    /// Live pairs (and connects in flight) a bridge accepts; more are closed at once, which a client sees as a refused connection.
    var maxPairs = 128
    /// While one side is half-closed, how often the other client is checked for having gone away completely.
    var halfClosePollSeconds: TimeInterval = 2
    /// A vsock connect that has not answered by then is abandoned and the client closed.
    var connectTimeoutSeconds: TimeInterval = 10
}

/// Counts live pairs, remembers them so shutdown can close them, and refuses past the limit.
final class BridgeSlots {
    let limits: BridgeLimits
    private let lock = NSLock()
    private var reserved = 0
    private var pairs: [ObjectIdentifier: BridgePair] = [:]
    init(limits: BridgeLimits = BridgeLimits()) { self.limits = limits }

    /// Reserves a slot for a connection about to be made, or false at the limit.
    func acquire() -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard reserved < limits.maxPairs else { return false }
        reserved += 1
        return true
    }
    func release() { lock.lock(); reserved = max(0, reserved - 1); lock.unlock() }
    fileprivate func add(_ p: BridgePair) { lock.lock(); pairs[ObjectIdentifier(p)] = p; lock.unlock() }
    fileprivate func remove(_ p: BridgePair) { lock.lock(); pairs[ObjectIdentifier(p)] = nil; reserved = max(0, reserved - 1); lock.unlock() }
    var live: Int { lock.lock(); defer { lock.unlock() }; return reserved }
    /// Shutdown: every pair still open is closed (their descriptors are closed by their own cancel handlers).
    func closeAll(_ reason: String) {
        lock.lock(); let all = Array(pairs.values); lock.unlock()
        all.forEach { $0.closeAsync(reason) }
    }
}

/// A vsock connect that resolves exactly once: by its completion or by its timeout, whichever comes first.
final class PendingConnect {
    private let lock = NSLock()
    private var done = false
    func claim() -> Bool { lock.lock(); defer { lock.unlock() }; if done { return false }; done = true; return true }
}

/// One verdict on the guest's vsock transport, shared by the Docker bridge, the management bridge and port sync. VZVirtioSocketDevice.connect
/// has no cancel or timeout: with the guest dead (its init panicked) completions never arrive or arrive as resets, and the framework keeps
/// one descriptor per connect that never completed. After `threshold` failures with no success in between only one probe per
/// `probeInterval` is let through; the rest are refused without touching the framework. Any success closes the gate.
final class VsockHealth {
    private let lock = NSLock()
    private var stalls = 0
    private var nextProbe = Date.distantPast
    let threshold: Int, probeInterval: TimeInterval
    init(threshold: Int = 3, probeInterval: TimeInterval = 15) { self.threshold = threshold; self.probeInterval = probeInterval }
    func allow(_ now: Date = Date()) -> Bool {
        lock.lock(); defer { lock.unlock() }
        if stalls < threshold { return true }
        if now >= nextProbe { nextProbe = now.addingTimeInterval(probeInterval); return true }
        return false
    }
    func succeeded() { lock.lock(); stalls = 0; lock.unlock() }
    func failed() { lock.lock(); stalls += 1; lock.unlock() }
    var degraded: Bool { lock.lock(); defer { lock.unlock() }; return stalls >= threshold }
}
let vsockHealth = VsockHealth()

/// `connected`: something accepts on the host socket (the bridge does, even with the guest dead). `answered`: the Docker daemon behind it
/// replied to `GET /_ping`, which is the only thing that makes Docker ready.
func dockerSocketProbe(path: String, timeoutSeconds: Int = 2) -> (connected: Bool, answered: Bool) {
    if !FileManager.default.fileExists(atPath: path) { return (false, false) }
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return (false, false) }
    defer { close(fd) }
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
    _ = path.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
    let len = socklen_t(MemoryLayout<sockaddr_un>.size)
    let ret = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(fd, sp, len) } }
    if ret != 0 { return (false, false) }
    var tv = timeval(tv_sec: timeoutSeconds, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    var on: Int32 = 1; setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
    let req = "GET /_ping HTTP/1.0\r\nHost: localhost\r\n\r\n"
    if req.withCString({ write(fd, $0, strlen($0)) }) < 0 { return (true, false) }
    var buf = [UInt8](repeating: 0, count: 256)
    let n = read(fd, &buf, buf.count)
    return (true, n > 0 && dockerPingAnswered(String(decoding: buf[0..<n], as: UTF8.self)))
}

func dockerPingAnswered(_ response: String) -> Bool { response.hasPrefix("HTTP/1.") && response.prefix(32).contains(" 200") }

/// Docker is ready only when the daemon answered `/_ping`. A log that once said HARPOON_RUNNING, with a socket that cannot be probed
/// (sandboxed callers), is reported as unverified and never as ready; a socket that connects but does not answer is neither.
func dockerReadiness(sockExists: Bool, probe: (connected: Bool, answered: Bool), logHasRunning: Bool) -> (ready: Bool, unverified: Bool) {
    (probe.answered, !probe.answered && sockExists && logHasRunning && !probe.connected)
}

/// The kernel's own panic line from the guest console, or nil. Anchored to the console's `[ seconds]` prefix so text a workload prints
/// cannot pass for it.
func guestPanicLine(in serial: String) -> String? {
    guard let re = try? NSRegularExpression(pattern: "^\\[ *\\d+\\.\\d+\\] Kernel panic - not syncing:.*$", options: [.anchorsMatchLines]),
          let m = re.firstMatch(in: serial, range: NSRange(serial.startIndex..., in: serial)), let r = Range(m.range, in: serial) else { return nil }
    return String(serial[r].prefix(200))
}

/// accept() failures that mean "out of descriptors/buffers right now": the listener must pause, not spin on the same failure.
func acceptShouldBackOff(_ err: Int32) -> Bool { err == EMFILE || err == ENFILE || err == ENOBUFS || err == ENOMEM }

final class BridgePair {
    let id: Int
    private let tag: String
    private let clientFd: Int32
    private let peerFd: Int32
    private let closePeer: () -> Void
    private let log: (String) -> Void
    private let limits: BridgeLimits
    private weak var slots: BridgeSlots?
    private let queue: DispatchQueue
    private var clientSource: DispatchSourceRead?
    private var peerSource: DispatchSourceRead?
    private var monitor: DispatchSourceTimer?
    private var clientEOF = false, peerEOF = false, closed = false
    private var cancelsPending = 2
    private let stopLock = NSLock()
    private var stopReason: String?   // set from any thread by closeAsync; read by a blocked write, which owns the queue and would never see a queued close
    private var selfRetain: BridgePair?   // alive until both descriptors are closed, whoever else holds it

    /// Bytes read from the client. The default forwards them untouched; the Docker bridge installs its HTTP-aware forwarder.
    /// Called on the pair's queue; use `writePeer` and `close` from inside.
    var onClientData: ((BridgePair, [UInt8]) -> Void)?

    /// `closePeer` closes the vsock connection (VZVirtioSocketConnection.close, or close(fd) in tests); it runs once, after the peer's
    /// dispatch source has been cancelled. The client descriptor is closed by this object, also only after its source is cancelled.
    init(id: Int, tag: String, clientFd: Int32, peerFd: Int32, slots: BridgeSlots?, limits: BridgeLimits = BridgeLimits(),
         log: @escaping (String) -> Void, closePeer: @escaping () -> Void) {
        self.id = id; self.tag = tag; self.clientFd = clientFd; self.peerFd = peerFd
        self.slots = slots; self.limits = limits; self.log = log; self.closePeer = closePeer
        self.queue = DispatchQueue(label: "harpoon.bridge.\(tag).\(id)")
    }

    func start() {
        for fd in [clientFd, peerFd] {
            let flags = fcntl(fd, F_GETFL, 0); if flags >= 0 { _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK) }
            var on: Int32 = 1; setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        }
        let cs = DispatchSource.makeReadSource(fileDescriptor: clientFd, queue: queue)
        let ps = DispatchSource.makeReadSource(fileDescriptor: peerFd, queue: queue)
        clientSource = cs; peerSource = ps
        cs.setEventHandler { [weak self] in self?.clientReadable() }
        ps.setEventHandler { [weak self] in self?.peerReadable() }
        cs.setCancelHandler { [weak self] in self?.sourceCancelled() }
        ps.setCancelHandler { [weak self] in self?.sourceCancelled() }
        selfRetain = self
        slots?.add(self)
        cs.resume(); ps.resume()
    }

    /// Safe from any thread, including while the pair's queue is stuck in a blocked write: the flag ends that write within one poll tick.
    func closeAsync(_ reason: String) {
        stopLock.lock(); if stopReason == nil { stopReason = reason }; stopLock.unlock()
        queue.async { self.close(reason) }
    }
    private var stopRequested: String? { stopLock.lock(); defer { stopLock.unlock() }; return stopReason }

    /// Idempotent; call on the pair's queue. Cancels both sources; the cancel handlers close the descriptors.
    func close(_ reason: String) {
        if closed { return }
        closed = true
        monitor?.cancel(); monitor = nil
        log("\(tag)_CLOSE \(id) \(reason) clientFd=\(clientFd) vsockFd=\(peerFd)")
        clientSource?.cancel(); peerSource?.cancel()
    }

    private func sourceCancelled() {
        cancelsPending -= 1
        guard cancelsPending == 0 else { return }
        Darwin.close(clientFd)
        closePeer()
        slots?.remove(self)
        onClientData = nil      // the hook may capture this pair
        selfRetain = nil
    }

    /// Write everything or give up. A reader that is merely slow (a paused `docker logs -f`, a busy guest) is waited for with poll(), no
    /// busy loop; the wait ends when the write completes or when the CLIENT has gone away entirely, which is checked while blocked so a
    /// vanished client cannot pin the pair behind a write that will never finish.
    private func writeAll(_ fd: Int32, _ bytes: UnsafeRawBufferPointer, to side: String) -> Bool {
        var off = 0
        while off < bytes.count {
            let w = write(fd, bytes.baseAddress!.advanced(by: off), bytes.count - off)
            if w > 0 { off += w; continue }
            if w < 0 && errno == EINTR { continue }
            if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) {
                var p = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
                _ = poll(&p, 1, 100)
                if let why = stopRequested { close(why); return false }
                if unixPeerClosed(clientFd) { close("client closed while a \(side) write was blocked"); return false }
                continue
            }
            close(errno == EPIPE ? "\(side) EPIPE" : "\(side) write \(String(cString: strerror(errno)))")
            return false
        }
        return true
    }
    func writePeer(_ bytes: [UInt8]) -> Bool { bytes.withUnsafeBytes { writeAll(peerFd, $0, to: "vsock") } }
    func writePeer(_ data: Data) -> Bool { data.withUnsafeBytes { writeAll(peerFd, $0, to: "vsock") } }
    func writeClient(_ bytes: [UInt8]) -> Bool { bytes.withUnsafeBytes { writeAll(clientFd, $0, to: "client") } }

    private func clientReadable() {
        if closed { return }
        var buf = [UInt8](repeating: 0, count: 8192)
        let n = read(clientFd, &buf, buf.count)
        if n == 0 {
            clientEOF = true; clientSource?.cancel()
            shutdown(peerFd, SHUT_WR)
            log("\(tag)_CLIENT_EOF \(id)")
            if peerEOF { close("both EOF") } else { watchClient() }
            return
        }
        if n < 0 {
            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
            close("client read \(String(cString: strerror(errno)))"); return
        }
        let chunk = Array(buf[0..<n])
        if let hook = onClientData { hook(self, chunk) } else { _ = writePeer(chunk) }
    }

    private func peerReadable() {
        if closed { return }
        var buf = [UInt8](repeating: 0, count: 8192)
        let n = read(peerFd, &buf, buf.count)
        if n == 0 {
            peerEOF = true; peerSource?.cancel()
            shutdown(clientFd, SHUT_WR)
            log("\(tag)_VSOCK_EOF \(id)")
            if clientEOF { close("both EOF") } else { watchClient() }
            return
        }
        if n < 0 {
            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return }
            close("vsock read \(String(cString: strerror(errno)))"); return
        }
        _ = writeClient(Array(buf[0..<n]))
    }

    /// One side is finished and the other is not. A client that only half-closed keeps its answer coming; one that is gone entirely
    /// would otherwise hold both descriptors until the guest happened to close, which a silent guest never does.
    private func watchClient() {
        guard monitor == nil, !closed else { return }
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now() + limits.halfClosePollSeconds, repeating: limits.halfClosePollSeconds)
        t.setEventHandler { [weak self] in
            if let self, !self.closed, unixPeerClosed(self.clientFd) { self.close("client closed while the guest side was still open") }
        }
        monitor = t
        t.resume()
    }
}
