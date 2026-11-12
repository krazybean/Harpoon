import Foundation

// The guest console is untrusted, unbounded input. Nothing here reads a file back: a pipe is drained by one thread, scanned
// incrementally as bytes, kept in a bounded tail, and persisted through a bounded, rate-limited, rotating log.

/// Finds the boot markers in a byte stream that arrives in arbitrary chunks. Pure: no I/O, memory bounded by `carryLimit` + one chunk.
struct SerialScanner {
    static let carryLimit = 1024
    static let failedReasonBytes = 400
    private static let ready = Array("HARPOON_DOCKER_READY".utf8)
    private static let mgmt = Array("HARPOON_MGMT_READY".utf8)
    private static let failed = Array("HARPOON_DOCKER_FAILED".utf8)
    private static let panic = Array("Kernel panic - not syncing".utf8)

    private(set) var dockerReady = false
    private(set) var mgmtReady = false
    private(set) var panicLine: String?
    private var failedCapture: [UInt8]?
    private var carry: [UInt8] = []

    /// Text after HARPOON_DOCKER_FAILED (up to 400 bytes, lossily decoded); non-nil as soon as the marker is seen.
    var dockerFailedReason: String? { failedCapture.map { String(decoding: $0, as: UTF8.self) } }

    private static func find(_ needle: [UInt8], in hay: [UInt8]) -> Int? {
        hay.withUnsafeBytes { h in needle.withUnsafeBytes { n in
            guard let base = h.baseAddress, let p = memmem(base, h.count, n.baseAddress!, n.count) else { return nil }
            return base.distance(to: p)
        } }
    }

    mutating func feed(_ chunk: [UInt8]) {
        if var cap = failedCapture, cap.count < Self.failedReasonBytes {
            cap += chunk.prefix(Self.failedReasonBytes - cap.count)
            failedCapture = cap
        }
        let window = carry + chunk // CARRY: a marker split across two reads is whole in the window
        if !dockerReady, Self.find(Self.ready, in: window) != nil { dockerReady = true }
        if !mgmtReady, Self.find(Self.mgmt, in: window) != nil { mgmtReady = true }
        if failedCapture == nil, let i = Self.find(Self.failed, in: window) {
            failedCapture = Array(window[(i + Self.failed.count)...].prefix(Self.failedReasonBytes))
        }
        // The line must be complete (or long enough) so a panic split mid-message is reported whole, not truncated.
        if panicLine == nil, let i = Self.find(Self.panic, in: window), window[i...].contains(10) || window.count - i >= 200,
           let line = guestPanicLine(in: String(decoding: window, as: UTF8.self)) {
            panicLine = line
        }
        carry = Array(window.suffix(Self.carryLimit))
    }
}

/// An append-only log bounded to two files (`path` and `path.1`): at most 2 × maxBytes is ever retained, whatever the writer does.
final class RotatingLog {
    let path: String
    let maxBytes: Int
    private var fd: Int32 = -1
    private var size = 0

    init(path: String, maxBytes: Int) { self.path = path; self.maxBytes = max(1, maxBytes) }

    /// Starts a fresh log (as the previous whole-file attachment did) and drops this log's own stale rotation.
    func open() -> Int32 {
        close()
        unlink(path + ".1")
        return create()
    }

    private func create() -> Int32 {
        fd = Darwin.open(path, O_CREAT | O_TRUNC | O_WRONLY | O_CLOEXEC, 0o600)
        size = 0
        return fd < 0 ? errno : 0
    }

    func close() { if fd >= 0 { Darwin.close(fd); fd = -1 } }

    /// 0 on success, else an errno. Never writes more than `maxBytes` into one file.
    func append(_ bytes: UnsafeRawBufferPointer) -> Int32 {
        var off = 0
        while off < bytes.count {
            if fd < 0 { let e = create(); if e != 0 { return e } }
            if size >= maxBytes { // ROTATE
                Darwin.close(fd); fd = -1
                if rename(path, path + ".1") != 0 { return errno }
                let e = create(); if e != 0 { return e }
            }
            let take = min(maxBytes - size, bytes.count - off)
            let n = Darwin.write(fd, bytes.baseAddress! + off, take)
            if n < 0 { if errno == EINTR { continue }; return errno }
            off += n; size += n
        }
        return 0
    }
}

/// Drains the guest console pipe on its own thread. The reader never waits on the VM, the log or the main queue, so a console flood
/// can neither back up into the guest nor grow host memory: bytes are scanned, then written within a rate budget or counted as dropped.
final class SerialCollector {
    struct Snapshot {
        var dockerReady = false, mgmtReady = false
        var dockerFailedReason: String?
        var panicLine: String?
        var bytesSeen = 0, bytesDropped = 0
        var storageError: String?
        var readerFailure: String?
        var overflowing = false
    }

    static let readChunk = 64 * 1024
    let tailLimit: Int
    private let readFd: Int32
    private let logFile: RotatingLog
    private let burstBytes: Double
    private let refillPerSecond: Double
    private let clock: () -> Double
    private let log: (String) -> Void
    private let lock = NSLock()
    private var scanner = SerialScanner()
    private var snap = Snapshot()
    private var tailBuf: [UInt8] = []
    private var tokens: Double
    private var lastRefill: Double
    private var lastOverflowNote = -Double.infinity
    private var stopping = false
    private var thread: Thread?

    /// - fileLimit: bytes kept per file (retained total is at most twice this).
    /// - burstBytes / refillPerSecond: the write budget. Healthy boots fit in the burst; a sustained flood is written at the refill rate.
    init(readFd: Int32, logPath: String, fileLimit: Int = 4 * 1024 * 1024, tailLimit: Int = 64 * 1024,
         burstBytes: Int = 2 * 1024 * 1024, refillPerSecond: Int = 64 * 1024,
         clock: @escaping () -> Double = { Double(DispatchTime.now().uptimeNanoseconds) / 1e9 }, log: @escaping (String) -> Void) {
        self.readFd = readFd; self.tailLimit = tailLimit; self.clock = clock; self.log = log
        self.logFile = RotatingLog(path: logPath, maxBytes: fileLimit)
        self.burstBytes = Double(burstBytes); self.refillPerSecond = Double(refillPerSecond)
        self.tokens = Double(burstBytes); self.lastRefill = clock()
        let e = logFile.open()
        if e != 0 { snap.storageError = "cannot open \(logPath): \(String(cString: strerror(e)))" }
    }

    func start() {
        let t = Thread { [self] in run() }
        t.name = "harpoon-serial-collector"; t.qualityOfService = .utility
        thread = t; t.start()
    }

    /// Ends the reader at its next poll (<= 250 ms). The caller owns and closes the descriptors.
    func stop() { lock.lock(); stopping = true; lock.unlock() }

    func snapshot() -> Snapshot {
        lock.lock(); defer { lock.unlock() }
        var s = snap
        s.dockerReady = scanner.dockerReady; s.mgmtReady = scanner.mgmtReady
        s.dockerFailedReason = scanner.dockerFailedReason; s.panicLine = scanner.panicLine
        return s
    }

    func tail(_ n: Int) -> String { lock.lock(); defer { lock.unlock() }; return String(decoding: tailBuf.suffix(n), as: UTF8.self) }

    private func run() {
        var buf = [UInt8](repeating: 0, count: Self.readChunk)
        while true {
            lock.lock(); let done = stopping; lock.unlock()
            if done { return }
            var p = pollfd(fd: readFd, events: Int16(POLLIN), revents: 0)
            let r = poll(&p, 1, 250)
            if r < 0 { if errno == EINTR { continue }; fail("poll: \(String(cString: strerror(errno)))"); return }
            if r == 0 { continue }
            let n = buf.withUnsafeMutableBytes { read(readFd, $0.baseAddress, $0.count) }
            if n > 0 { consume(Array(buf[0..<n])) }
            else if n == 0 { fail("console pipe closed"); return }
            else if errno != EINTR && errno != EAGAIN { fail("read: \(String(cString: strerror(errno)))"); return }
        }
    }

    private func fail(_ why: String) {
        lock.lock(); let expected = stopping; if !expected { snap.readerFailure = why }; lock.unlock()
        if !expected { log("HARPOON_SERIAL_COLLECTOR_FAILED \(why)") }
    }

    /// One chunk of guest output. Internal so tests can drive it without a pipe.
    func consume(_ chunk: [UInt8]) {
        lock.lock()
        let wasPanic = scanner.panicLine != nil, wasFailed = scanner.dockerFailedReason != nil
        scanner.feed(chunk)
        snap.bytesSeen += chunk.count
        tailBuf += chunk.suffix(tailLimit)
        if tailBuf.count > tailLimit * 2 { tailBuf.removeFirst(tailBuf.count - tailLimit) }
        let now = clock()
        tokens = min(burstBytes, tokens + (now - lastRefill) * refillPerSecond); lastRefill = now
        let allowed = min(chunk.count, Int(tokens))
        tokens -= Double(allowed)
        var notes: [String] = []
        if allowed < chunk.count {
            snap.bytesDropped += chunk.count - allowed
            snap.overflowing = true
            if now - lastOverflowNote >= 10 { // rate-limited so the note itself cannot flood the log
                lastOverflowNote = now
                notes.append("HARPOON_SERIAL_OVERFLOW dropped=\(snap.bytesDropped) seen=\(snap.bytesSeen) writeBudget=\(Int(refillPerSecond))B/s")
            }
        } else { snap.overflowing = false }
        if !wasPanic, let l = scanner.panicLine { notes.append("HARPOON_GUEST_PANIC_LINE \(l)") }
        if !wasFailed, scanner.dockerFailedReason != nil { notes.append("HARPOON_DOCKER_FAILED_SEEN") }
        var err: Int32 = 0
        if allowed > 0 { err = chunk.withUnsafeBytes { logFile.append(UnsafeRawBufferPointer(rebasing: $0[0..<allowed])) } }
        for n in notes where err == 0 { // one-shot evidence lines bypass the budget; they are few and bounded by rotation
            let line = Array((n + "\n").utf8)
            err = line.withUnsafeBytes { logFile.append($0) }
        }
        var newStorageError: String?
        if err != 0, snap.storageError == nil { snap.storageError = "log write failed: \(String(cString: strerror(err)))"; newStorageError = snap.storageError }
        lock.unlock()
        for n in notes where n.hasPrefix("HARPOON_SERIAL_OVERFLOW") { log(n) }
        if let e = newStorageError { log("HARPOON_SERIAL_STORAGE_FAILED \(e) (console is still drained and scanned)") }
    }
}
