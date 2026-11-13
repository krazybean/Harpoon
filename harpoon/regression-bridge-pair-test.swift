import Foundation
import Darwin

// Bridge pair lifecycle with socketpair test doubles: no VM, no vsock, no Docker, no real socket path. Counts this process's open
// descriptors across repeated connections to prove nothing accumulates, and that a half-closed client still gets its answer.

func require(_ cond: @autoclosure () -> Bool, _ message: String) {
    if !cond() { fputs("bridge pair FAIL: \(message)\n", stderr); exit(1) }
}
func openFds() -> Int { (try? FileManager.default.contentsOfDirectory(atPath: "/dev/fd").count) ?? -1 }
func waitFor(_ seconds: Double, _ cond: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end { if cond() { return true }; usleep(20_000) }
    return cond()
}
func pairOfSockets() -> (Int32, Int32) { var f: [Int32] = [0, 0]; require(socketpair(AF_UNIX, SOCK_STREAM, 0, &f) == 0, "socketpair"); return (f[0], f[1]) }
func readString(_ fd: Int32, timeout: Double = 3) -> String? {
    var p = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
    guard poll(&p, 1, Int32(timeout * 1000)) > 0 else { return nil }
    var buf = [UInt8](repeating: 0, count: 4096)
    let n = read(fd, &buf, buf.count)
    return n > 0 ? String(decoding: buf[0..<n], as: UTF8.self) : (n == 0 ? "" : nil)
}
func send(_ fd: Int32, _ s: String) { _ = s.withCString { write(fd, $0, strlen($0)) } }

let fast = BridgeLimits(maxPairs: 64, halfClosePollSeconds: 0.1, connectTimeoutSeconds: 1)
var logs: [String] = []
let logLock = NSLock()
let log: (String) -> Void = { logLock.lock(); logs.append($0); logLock.unlock() }

/// A bridged pair: (client end the test drives, guest end the test drives, the pair).
func bridged(_ slots: BridgeSlots? = nil, id: Int = 0) -> (client: Int32, guest: Int32, pair: BridgePair) {
    let (cEnd, cfd) = pairOfSockets(), (vEnd, vfd) = pairOfSockets()
    let pair = BridgePair(id: id, tag: "TEST", clientFd: cfd, peerFd: vfd, slots: slots, limits: fast, log: log, closePeer: { close(vfd) })
    pair.start()
    return (cEnd, vEnd, pair)
}
func settle(to baseline: Int, _ what: String) {
    require(waitFor(5) { openFds() == baseline }, "\(what): descriptors did not return to \(baseline) (now \(openFds()))")
}

signal(SIGPIPE, SIG_IGN)
let base = openFds()
require(base > 0, "cannot count descriptors")

// peer-closed detection: a half-close is not a close
do {
    let (a, b) = pairOfSockets(); require(!unixPeerClosed(b), "an open peer is not closed")
    shutdown(a, SHUT_WR); require(!unixPeerClosed(b), "a half-closed peer is not closed")
    close(a); require(unixPeerClosed(b), "a closed peer is closed"); close(b)
}
require(acceptShouldBackOff(EMFILE) && acceptShouldBackOff(ENFILE) && !acceptShouldBackOff(EAGAIN) && !acceptShouldBackOff(ECONNABORTED), "accept backoff classification")
do { let p = PendingConnect(); require(p.claim() && !p.claim(), "a connect resolves exactly once") }
do { let s = BridgeSlots(limits: BridgeLimits(maxPairs: 2)); require(s.acquire() && s.acquire() && !s.acquire(), "slots refuse past the limit"); s.release(); require(s.acquire(), "a released slot is reusable") }
settle(to: base, "helpers")

// 1. an ordinary request and answer, then both close
do {
    let (c, g, _) = bridged()
    send(c, "ping"); require(readString(g) == "ping", "request reaches the guest")
    send(g, "pong"); require(readString(c) == "pong", "answer reaches the client")
    close(c); close(g)
    settle(to: base, "ordinary exchange")
}

// 2. a client that half-closes after its request still receives a late answer, and the pair then closes
do {
    let (c, g, _) = bridged()
    send(c, "request"); shutdown(c, SHUT_WR)
    require(readString(g) == "request", "half-closed request reaches the guest")
    require(readString(g) == "", "the guest sees the client's EOF")
    usleep(400_000)                      // longer than several monitor polls: a half-closed client must NOT be reaped
    send(g, "late answer"); close(g)
    require(readString(c) == "late answer", "the answer after a half-close is delivered")
    require(readString(c) == "", "the client then sees EOF")
    close(c)
    settle(to: base, "half-close with late answer")
}

// 3. the leak: clients give up (probe timed out, closed) while the guest stays silent and open
do {
    let n = 100
    var guests: [Int32] = []
    for i in 0..<n { let (c, g, _) = bridged(id: i); send(c, "probe"); close(c); guests.append(g) }
    // the pairs must release both of their descriptors even though no guest ever closes or answers
    require(waitFor(8) { openFds() == base + n }, "silent guest: \(openFds() - base - n) descriptors still held by abandoned pairs (of \(2 * n))")
    guests.forEach { close($0) }
    settle(to: base, "silent guest, abandoned clients")
}

// 4. the guest end is already gone (EPIPE on the first write)
do {
    for i in 0..<50 { let (c, g, _) = bridged(id: i); close(g); usleep(1000); send(c, "hello"); close(c) }
    settle(to: base, "guest gone before the request")
}

// 5. both ends close at the same instant, many times (the old handlers raced on shared flags and could close twice)
do {
    for i in 0..<300 {
        let (c, g, _) = bridged(id: i)
        let grp = DispatchGroup()
        DispatchQueue.global().async(group: grp) { close(c) }
        DispatchQueue.global().async(group: grp) { close(g) }
        grp.wait()
    }
    settle(to: base, "simultaneous close")
}

// 6. a guest that stops reading is waited for (backpressure, not a failure) until the client goes away; then the pair closes
do {
    let (c, g, _) = bridged()
    let blob = [UInt8](repeating: 0x41, count: 1 << 20)
    let writer = DispatchQueue(label: "test.writer")
    writer.async { for _ in 0..<64 { if blob.withUnsafeBytes({ write(c, $0.baseAddress!, $0.count) }) < 0 && errno != EAGAIN && errno != EINTR { break } } }
    usleep(1_500_000)
    require(!logs.contains { $0.contains("TEST_CLOSE") && $0.contains("write") }, "a slow reader must not be cut off by a timer")
    close(c)                                   // the client leaves while the pair is blocked writing to the silent guest
    require(waitFor(8) { logs.contains { $0.contains("client closed while a vsock write was blocked") } }, "a vanished client frees a pair stuck in a blocked write")
    writer.sync {}
    close(g)
    settle(to: base, "stalled guest")
}

// 6b. shutdown while a write is blocked and the client is still connected: the close must not wait behind the write
do {
    let slots = BridgeSlots(limits: fast)
    require(slots.acquire(), "slot")
    let (c, g, _) = bridged(slots, id: 900)
    let blob = [UInt8](repeating: 0x42, count: 1 << 20)
    let writer = DispatchQueue(label: "test.writer2")
    writer.async { for _ in 0..<64 { if blob.withUnsafeBytes({ write(c, $0.baseAddress!, $0.count) }) < 0 && errno != EAGAIN && errno != EINTR { break } } }
    usleep(1_000_000)                          // guest never reads: the pair is parked inside writeAll; client stays connected
    require(slots.live == 1, "pair should still be live while blocked")
    let t0 = Date()
    slots.closeAll("test shutdown during blocked write")
    require(waitFor(3) { slots.live == 0 }, "shutdown did not release a pair blocked in a write")
    require(Date().timeIntervalSince(t0) < 2, "shutdown of a blocked pair was not bounded")
    require(logs.contains { $0.contains("test shutdown during blocked write") }, "close reason recorded")
    close(c); close(g); writer.sync {}
    settle(to: base, "shutdown during blocked write")
}

// 7. shutdown closes every live pair
do {
    let slots = BridgeSlots(limits: fast)
    var ends: [Int32] = []
    for i in 0..<20 { require(slots.acquire(), "slot"); let (c, g, _) = bridged(slots, id: i); ends += [c, g] }
    require(slots.live == 20, "twenty live pairs, saw \(slots.live)")
    slots.closeAll("test shutdown")
    require(waitFor(5) { slots.live == 0 }, "closeAll leaves pairs behind (\(slots.live))")
    ends.forEach { close($0) }
    settle(to: base, "closeAll")
}

// 7b. guest-down gate: after repeated failures only one probe per interval; one success reopens it
do {
    let h = VsockHealth(threshold: 3, probeInterval: 15), t0 = Date()
    require(h.allow(t0) && !h.degraded, "healthy at the start")
    h.failed(); h.failed(); require(h.allow(t0), "below the threshold every connect is allowed")
    h.failed(); require(h.degraded, "three failures degrade")
    require(h.allow(t0), "the first attempt after degrading is the probe")
    var refused = 0; for _ in 0..<500 where !h.allow(t0.addingTimeInterval(5)) { refused += 1 }
    require(refused == 500, "a storm inside the probe interval must not reach the framework (\(500 - refused) got through)")
    require(h.allow(t0.addingTimeInterval(16)) && !h.allow(t0.addingTimeInterval(17)), "one probe per interval")
    h.failed(); require(h.degraded, "a failed probe keeps it degraded")
    h.succeeded(); require(!h.degraded && h.allow(t0.addingTimeInterval(18)) && h.allow(t0.addingTimeInterval(18)), "a success reopens the gate")
}

// 7c. Docker readiness is an answered /_ping, not a socket that accepts (what a bridge to a dead guest does)
do {
    require(dockerPingAnswered("HTTP/1.1 200 OK\r\nApi-Version: 1.51\r\n\r\nOK"), "200 is ready")
    require(!dockerPingAnswered("HTTP/1.1 500 Internal\r\n\r\n") && !dockerPingAnswered(""), "non-200 or empty is not ready")
    let dir = NSTemporaryDirectory() + "hp-ping-\(getpid())"; try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: dir) }
    let stopLock = NSLock()
    func listener(_ path: String, reply: String?) -> () -> Void {
        unlink(path)
        let l = socket(AF_UNIX, SOCK_STREAM, 0); var a = sockaddr_un(); a.sun_family = sa_family_t(AF_UNIX)
        _ = path.withCString { src in withUnsafeMutablePointer(to: &a.sun_path) { strncpy(UnsafeMutableRawPointer($0).assumingMemoryBound(to: CChar.self), src, 100) } }
        _ = withUnsafePointer(to: a) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(l, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        listen(l, 4)
        let done = DispatchSemaphore(value: 0); var halt = false
        DispatchQueue.global().async {
            var held: [Int32] = []
            while true {
                stopLock.lock(); let h = halt; stopLock.unlock(); if h { break }
                var p = pollfd(fd: l, events: Int16(POLLIN), revents: 0)
                if poll(&p, 1, 50) <= 0 { continue }
                let c = accept(l, nil, nil); if c < 0 { continue }
                if let r = reply { _ = r.withCString { write(c, $0, strlen($0)) }; close(c) } else { held.append(c) }
            }
            held.forEach { close($0) }; close(l); done.signal()
        }
        return { stopLock.lock(); halt = true; stopLock.unlock(); done.wait() }
    }
    let sock = dir + "/d.sock"
    let silent = listener(sock, reply: nil)         // accepts and never answers: the bridge in front of a dead guest
    let t0 = Date(); let r1 = dockerSocketProbe(path: sock, timeoutSeconds: 1)
    require(r1.connected && !r1.answered, "an accepting but silent socket is connected, not ready")
    require(Date().timeIntervalSince(t0) < 3, "the probe is bounded")
    silent()
    let live = listener(sock, reply: "HTTP/1.0 200 OK\r\n\r\nOK")
    let r2 = dockerSocketProbe(path: sock); require(r2.connected && r2.answered, "an answering daemon is ready")
    live()
    let r3 = dockerSocketProbe(path: dir + "/none.sock"); require(!r3.connected && !r3.answered, "no socket is neither")
}

// 7d. readiness never comes from history: existing socket + stale running log + failed probe
do {
    typealias P = (connected: Bool, answered: Bool)
    let silent: P = (true, false), refused: P = (false, false), ok: P = (true, true)
    var r = dockerReadiness(sockExists: true, probe: silent, logHasRunning: true)
    require(!r.ready && !r.unverified, "socket accepts, daemon silent, log says RUNNING: not ready and not even unverified")
    r = dockerReadiness(sockExists: true, probe: refused, logHasRunning: true)
    require(!r.ready && r.unverified, "cannot probe, stale log: not ready, reported unverified")
    r = dockerReadiness(sockExists: true, probe: refused, logHasRunning: false)
    require(!r.ready && !r.unverified, "cannot probe, no log: nothing to report")
    r = dockerReadiness(sockExists: false, probe: refused, logHasRunning: true)
    require(!r.ready && !r.unverified, "no socket file: not unverified either")
    r = dockerReadiness(sockExists: true, probe: ok, logHasRunning: false)
    require(r.ready && !r.unverified, "an answered ping is ready whatever the log says")
}

// 7e. the guest kernel panic seen on the console (taken from the incident's serial log)
do {
    let incident = """
    HARPOON_SPIKE_OK
    HARPOON_SPIKE_OK
    [   87.633055] Kernel panic - not syncing: Attempted to kill init! exitcode=0x0000000b
    [   87.633189] CPU: 0 UID: 0 PID: 1 Comm: init Not tainted 6.12.110-0-virt #1-Alpine
    """
    let line = guestPanicLine(in: incident)
    require(line?.contains("Attempted to kill init! exitcode=0x0000000b") == true, "panic line found: \(line ?? "nil")")
    require(guestPanicLine(in: "HARPOON_SPIKE_OK\nHARPOON_DOCKER_READY\n") == nil, "a healthy console has none")
    require(guestPanicLine(in: "echo Kernel panic - not syncing: fake\n   Kernel panic - not syncing: indented") == nil, "text without the kernel's timestamp prefix is not a panic")
}

// 8. bounded over a long run of probes: descriptors stay flat, they do not creep up
do {
    var peak = base
    for i in 0..<2000 {
        let (c, g, _) = bridged(id: i)
        send(c, "probe"); _ = readString(g); close(c); close(g)
        if i % 100 == 0 { peak = max(peak, openFds()) }
    }
    settle(to: base, "2000 probes")
    require(peak < base + 60, "descriptors crept up during the run: peak \(peak - base) above the start")
}

print("bridge pair PASS (\(logs.count) log lines)")
