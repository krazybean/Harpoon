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
