import Foundation

// Behavioural tests for SerialScanner / RotatingLog / SerialCollector. No VM: the guest console is a pipe this test writes to.
// Driven (and mutation-checked) by regression-resource-safeguards.sh.

var failures = 0
func require(_ ok: Bool, _ what: String) {
    if ok { print("  ok   \(what)") } else { print("  FAIL \(what)"); failures += 1 }
}
func residentBytes() -> Int {
    var info = mach_task_basic_info(); var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
    let r = withUnsafeMutablePointer(to: &info) { $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count) } }
    return r == KERN_SUCCESS ? Int(info.resident_size) : 0
}
func fileSize(_ p: String) -> Int { ((try? FileManager.default.attributesOfItem(atPath: p)[.size]) as? Int) ?? 0 }
let tmp = NSTemporaryDirectory() + "hp-serial-test-\(getpid())"
try? FileManager.default.createDirectory(atPath: tmp, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(atPath: tmp) }
// A test that hangs is a failed test: a collector that stops draining would block the writer below forever.
DispatchQueue.global().asyncAfter(deadline: .now() + 90) { print("  FAIL watchdog: a flood did not drain (collector stalled)"); exit(2) }

print("scanner: markers split across reads")
do {
    let line = Array("noise\nHARPOON_DOCKER_READY\nmore\n".utf8)
    var all = true
    for cut in 0...line.count {
        var s = SerialScanner(); s.feed(Array(line[..<cut])); s.feed(Array(line[cut...]))
        if !s.dockerReady { all = false; print("    missed at cut \(cut)") }
    }
    require(all, "HARPOON_DOCKER_READY found at every split offset")
    var byByte = SerialScanner(); for b in line { byByte.feed([b]) }
    require(byByte.dockerReady, "found when fed one byte at a time")
    var m = SerialScanner(); for b in Array("x HARPOON_MGMT_READY".utf8) { m.feed([b]) }
    require(m.mgmtReady && !m.dockerReady, "MGMT marker independent of DOCKER marker")
}

print("scanner: binary / malformed UTF-8")
do {
    var s = SerialScanner()
    s.feed([0xFF, 0xFE, 0xC3] + Array("HARPOON_DOCKER_READY".utf8) + [0x80, 0x00, 0xE2, 0x82]) // invalid, NUL, truncated multibyte
    require(s.dockerReady, "marker amid invalid UTF-8 and NUL bytes")
    var f = SerialScanner(); f.feed(Array("HARPOON_DOCKER_FAILED boom ".utf8) + [0xC3, 0x28, 0xFF] + Array(" tail".utf8))
    require(f.dockerFailedReason?.hasPrefix(" boom ") == true, "failure reason survives invalid bytes (lossy decode, no crash)")
}

print("scanner: failure reason is bounded and split-safe")
do {
    var f = SerialScanner(); f.feed(Array("a HARPOON_DOCKER_FAI".utf8)); f.feed(Array("LED why".utf8))
    require(f.dockerFailedReason == " why", "marker split across reads; reason so far = \(f.dockerFailedReason ?? "nil")")
    f.feed(Array(String(repeating: "x", count: 2000).utf8))
    require(f.dockerFailedReason?.utf8.count == SerialScanner.failedReasonBytes, "reason capped at \(SerialScanner.failedReasonBytes) bytes")
}

print("scanner: oversized burst after a marker")
do {
    var s = SerialScanner(); s.feed(Array("HARPOON_DOCKER_READY\n".utf8))
    let junk = [UInt8](repeating: 0x41, count: 64 * 1024)
    for _ in 0..<200 { s.feed(junk) }
    require(s.dockerReady, "marker kept after 12.5 MiB of later output")
    var late = SerialScanner(); for _ in 0..<200 { late.feed(junk) }; late.feed(Array("HARPOON_MGMT_READY\n".utf8))
    require(late.mgmtReady, "marker found after a 12.5 MiB burst")
}

print("scanner: kernel panic detection")
do {
    let panic = "[   12.345678] Kernel panic - not syncing: Attempted to kill init! exitcode=0x0000000b\n"
    var s = SerialScanner()
    s.feed(Array("[   12.3] boot ok\n[   12.345678] Kernel panic - not syncing: Attempted to ki".utf8))
    require(s.panicLine == nil, "an unfinished panic line is not reported truncated")
    s.feed(Array("ll init! exitcode=0x0000000b\n".utf8))
    require(s.panicLine == String(panic.dropLast()), "panic line split across reads reported whole: \(s.panicLine ?? "nil")")
    var n = SerialScanner(); n.feed(Array("echo Kernel panic - not syncing: just text\n".utf8))
    require(n.panicLine == nil, "the words in ordinary output are not a console panic")
}

print("rotating log: hard bound on retained bytes")
do {
    let p = tmp + "/rot.log"; let l = RotatingLog(path: p, maxBytes: 64 * 1024)
    require(l.open() == 0, "opens")
    let chunk = [UInt8](repeating: 0x42, count: 10 * 1024)
    var ok = true
    for _ in 0..<1000 { if chunk.withUnsafeBytes({ l.append($0) }) != 0 { ok = false } } // 10 MiB offered
    let huge = [UInt8](repeating: 0x43, count: 500 * 1024) // a single write larger than the whole limit
    ok = ok && huge.withUnsafeBytes({ l.append($0) }) == 0
    l.close()
    require(ok, "appends succeed")
    require(fileSize(p) <= 64 * 1024 && fileSize(p + ".1") <= 64 * 1024, "each file <= limit (\(fileSize(p)), \(fileSize(p + ".1")))")
    require(fileSize(p) + fileSize(p + ".1") <= 128 * 1024, "retained total <= 2 x limit after ~10.5 MiB")
}

print("collector: flood through a pipe, memory and disk stay bounded, markers still seen")
do {
    var fds: [Int32] = [0, 0]; _ = pipe(&fds)
    let p = tmp + "/flood.log"
    var logLines: [String] = []; let logLock = NSLock()
    let c = SerialCollector(readFd: fds[0], logPath: p, fileLimit: 64 * 1024, tailLimit: 16 * 1024, burstBytes: 128 * 1024, refillPerSecond: 32 * 1024,
                            clock: { 0 }, log: { logLock.lock(); logLines.append($0); logLock.unlock() })
    c.start()
    func send(_ b: [UInt8]) { b.withUnsafeBytes { var off = 0; while off < $0.count { let n = write(fds[1], $0.baseAddress! + off, $0.count - off); if n > 0 { off += n } else if errno != EINTR { break } } } }
    send(Array("[ 1.0] booting\nHARPOON_DOCKER_READY\n".utf8))
    let junk = [UInt8](repeating: 0x2E, count: 256 * 1024)
    for _ in 0..<8 { send(junk) } // warm the allocator before measuring
    let rssBefore = residentBytes()
    let total = 50 * 1024 * 1024
    for _ in 0..<(total / junk.count) { send(junk) }
    send(Array("\nHARPOON_MGMT_READY\n[  99.0] Kernel panic - not syncing: flood test\n".utf8))
    let deadline = Date().addingTimeInterval(30)
    while Date() < deadline, c.snapshot().panicLine == nil { usleep(20_000) }
    let s = c.snapshot()
    let rssGrowth = residentBytes() - rssBefore
    require(s.dockerReady && s.mgmtReady, "markers before and after the flood were seen")
    require(s.panicLine?.contains("flood test") == true, "panic after the flood was seen")
    require(s.bytesSeen >= total, "every byte was drained (\(s.bytesSeen))")
    require(s.bytesDropped > total - 1024 * 1024, "the write budget dropped the excess (\(s.bytesDropped) dropped)")
    let disk = fileSize(p) + fileSize(p + ".1")
    require(disk <= 128 * 1024 + 4096, "retained diagnostics bounded: \(disk) bytes after 50 MiB")
    require(c.tail(Int.max).utf8.count <= 2 * 16 * 1024 + SerialCollector.readChunk, "in-memory tail bounded: \(c.tail(Int.max).utf8.count) bytes")
    require(rssGrowth < 16 * 1024 * 1024, "resident memory growth < 16 MiB over a 50 MiB flood (grew \(rssGrowth / 1024) KiB)")
    logLock.lock(); let overflowNotes = logLines.filter { $0.hasPrefix("HARPOON_SERIAL_OVERFLOW") }.count; logLock.unlock()
    require(overflowNotes == 1, "overflow is reported once per 10 s window, not per chunk (\(overflowNotes))")
    require(s.readerFailure == nil && s.storageError == nil, "no failure recorded for a plain flood")
    let kept = (try? String(contentsOfFile: p, encoding: .isoLatin1)) ?? ""
    require(kept.contains("HARPOON_GUEST_PANIC_LINE") || (try? String(contentsOfFile: p + ".1", encoding: .isoLatin1))?.contains("HARPOON_GUEST_PANIC_LINE") == true,
            "the panic evidence line is persisted even though the flood exhausted the write budget")
    c.stop(); close(fds[1]); close(fds[0])
}

print("collector: storage failure never stalls or blinds the console")
do {
    var fds: [Int32] = [0, 0]; _ = pipe(&fds)
    let c = SerialCollector(readFd: fds[0], logPath: tmp + "/no/such/dir/x.log", log: { _ in })
    c.start()
    let msg = Array(String(repeating: "x", count: 100_000).utf8) + Array("\nHARPOON_DOCKER_READY\n".utf8)
    msg.withUnsafeBytes { var off = 0; while off < $0.count { let n = write(fds[1], $0.baseAddress! + off, $0.count - off); if n > 0 { off += n } } }
    let deadline = Date().addingTimeInterval(10)
    while Date() < deadline, !c.snapshot().dockerReady { usleep(10_000) }
    let s = c.snapshot()
    require(s.storageError != nil, "storage failure is recorded: \(s.storageError ?? "nil")")
    require(s.dockerReady, "the console is still drained and scanned")
    c.stop(); close(fds[1]); close(fds[0])
}

print("collector: reader failure is recorded; an intentional stop is not a failure")
do {
    var a: [Int32] = [0, 0]; _ = pipe(&a)
    let c1 = SerialCollector(readFd: a[0], logPath: tmp + "/eof.log", log: { _ in }); c1.start()
    close(a[1]) // every writer gone
    let d = Date().addingTimeInterval(5); while Date() < d, c1.snapshot().readerFailure == nil { usleep(10_000) }
    require(c1.snapshot().readerFailure != nil, "console EOF is recorded as a collector failure: \(c1.snapshot().readerFailure ?? "nil")")
    var b: [Int32] = [0, 0]; _ = pipe(&b)
    let c2 = SerialCollector(readFd: b[0], logPath: tmp + "/stop.log", log: { _ in }); c2.start()
    c2.stop(); usleep(600_000); close(b[1])
    usleep(300_000)
    require(c2.snapshot().readerFailure == nil, "stop() then close is not reported as a failure")
}

print(failures == 0 ? "serial collector: PASS" : "serial collector: \(failures) FAILED")
exit(failures == 0 ? 0 : 1)
