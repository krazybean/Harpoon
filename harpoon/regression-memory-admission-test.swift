import Foundation

// Behavioural tests for MemoryAdmission, including real concurrent processes. No VM. The registry is a private temp directory.
// Child modes (re-executing this binary): `hold <dir> <mib> <delayMs>` reserves, prints "admitted"/"denied", then holds until stdin closes.

let args = CommandLine.arguments
if args.count >= 5, args[1] == "hold" {
    let delay = UInt32(args[4]) ?? 0
    let outcome = MemoryAdmission.reserve(requestedMiB: Int(args[3])!, hostMiB: 18432, namespace: "child-\(getpid())", directory: args[2], afterDecision: { usleep(delay * 1000) })
    switch outcome {
    case .admitted(let r): print("admitted"); fflush(stdout); _ = readLine(); r.release()
    case .denied: print("denied"); fflush(stdout)
    case .unavailable(let why): print("unavailable \(why)"); fflush(stdout)
    }
    exit(0)
}

var failures = 0
func require(_ ok: Bool, _ what: String) { if ok { print("  ok   \(what)") } else { print("  FAIL \(what)"); failures += 1 } }
let dir = NSTemporaryDirectory() + "hp-adm-test-\(getpid())"
defer { try? FileManager.default.removeItem(atPath: dir) }
func resFiles() -> Int { ((try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? []).filter { $0.hasSuffix(".res") }.count }
func admitted(_ o: MemoryAdmission.Outcome) -> MemoryAdmission.Reservation? { if case .admitted(let r) = o { return r }; return nil }
func denied(_ o: MemoryAdmission.Outcome) -> Bool { if case .denied = o { return true }; return false }
func spawn(_ mib: Int, delayMs: Int) -> (Process, Pipe, Pipe) {
    let p = Process(); p.executableURL = URL(fileURLWithPath: args[0]); p.arguments = ["hold", dir, "\(mib)", "\(delayMs)"]
    let out = Pipe(), inn = Pipe(); p.standardOutput = out; p.standardInput = inn
    try! p.run(); return (p, out, inn)
}
func firstLine(_ p: Pipe) -> String { String(decoding: p.fileHandleForReading.availableData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }

print("policy")
let host = 18432 // the 18 GiB host from the incident
require(MemoryAdmission.failure(requestedMiB: 4096, activeMiB: [], hostMiB: host) == nil, "one 4096 MiB guest fits an 18 GiB host")
require(MemoryAdmission.failure(requestedMiB: 4096, activeMiB: [4096], hostMiB: host) == nil, "two fit")
require(MemoryAdmission.failure(requestedMiB: 4096, activeMiB: [4096, 4096], hostMiB: host) != nil, "the third (the incident's three namespaces) is denied")
require(MemoryAdmission.failure(requestedMiB: 4096, activeMiB: [], hostMiB: 8192) == nil, "the default guest fits an 8 GiB Mac")
require(MemoryAdmission.failure(requestedMiB: 4096, activeMiB: [4096], hostMiB: 8192) != nil, "but not two on an 8 GiB Mac")
require(MemoryAdmission.failure(requestedMiB: 6088, activeMiB: [], hostMiB: 11000) == nil, "a request that exactly meets the budget (60% of 11000 = 6600 incl. 512 overhead) fits")
require(MemoryAdmission.failure(requestedMiB: 6089, activeMiB: [], hostMiB: 11000) != nil, "one MiB over the budget is denied")
require(MemoryAdmission.failure(requestedMiB: 3000, activeMiB: [3000], hostMiB: 11000) != nil, "every guest's own overhead counts, not just the new one's")
require(MemoryAdmission.failure(requestedMiB: 20000, activeMiB: [], hostMiB: host)?.contains("--memory") == true, "an oversized request is denied with a remedy")

print("registry: live vs stale, in one process")
do {
    let r1 = admitted(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "a", directory: dir))
    let r2 = admitted(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "b", directory: dir))
    require(r1 != nil && r2 != nil, "two namespaces admitted")
    require(denied(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "c", directory: dir)), "a third namespace sees both reservations and is denied")
    r1?.release()
    let r3 = admitted(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "c", directory: dir))
    require(r3 != nil, "releasing one frees its share")
    r2?.release(); r3?.release()
}

print("registry: a killed owner's reservation is stale and reclaimed")
do {
    let (c1, o1, i1) = spawn(4096, delayMs: 0); let (c2, o2, i2) = spawn(4096, delayMs: 0)
    require(firstLine(o1) == "admitted" && firstLine(o2) == "admitted", "two child engines hold live reservations")
    require(denied(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "p", directory: dir)), "live children count against the budget")
    kill(c1.processIdentifier, SIGKILL); c1.waitUntilExit() // no clean release: only the kernel drops the lock
    let r = admitted(MemoryAdmission.reserve(requestedMiB: 4096, hostMiB: host, namespace: "p", directory: dir))
    require(r != nil, "after SIGKILL of one owner its reservation no longer counts")
    require(resFiles() == 2, "the dead owner's file was reclaimed (\(resFiles()) files: 1 live child + this one)")
    r?.release(); try? i2.fileHandleForWriting.close(); _ = i1; c2.waitUntilExit()
}

print("registry: concurrent starts are atomic")
do {
    try? FileManager.default.removeItem(atPath: dir)
    let kids = (0..<8).map { _ in spawn(4096, delayMs: 300) } // 300 ms between deciding and registering: the window a missing lock would lose
    let results = kids.map { firstLine($0.1) }
    let yes = results.filter { $0 == "admitted" }.count, no = results.filter { $0 == "denied" }.count
    require(yes == 2 && no == 6, "8 simultaneous 4096 MiB starts on 18 GiB: exactly 2 admitted, 6 denied (admitted \(yes), denied \(no), other \(8 - yes - no))")
    for k in kids { try? k.2.fileHandleForWriting.close(); k.0.waitUntilExit() }
    require(resFiles() == 0, "all reservations released at exit")
}

print("registry: fails closed on an unsafe directory")
do {
    let bad = NSTemporaryDirectory() + "hp-adm-bad-\(getpid())"
    try? FileManager.default.createDirectory(atPath: bad, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o777])
    defer { try? FileManager.default.removeItem(atPath: bad) }
    if case .unavailable = MemoryAdmission.reserve(requestedMiB: 1024, hostMiB: host, namespace: "x", directory: bad) { require(true, "a group/world-writable registry is refused") }
    else { require(false, "a group/world-writable registry is refused") }
    let link = NSTemporaryDirectory() + "hp-adm-link-\(getpid())"
    try? FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: bad)
    defer { try? FileManager.default.removeItem(atPath: link) }
    if case .unavailable = MemoryAdmission.reserve(requestedMiB: 1024, hostMiB: host, namespace: "x", directory: link) { require(true, "a symlinked registry is refused") }
    else { require(false, "a symlinked registry is refused") }
}

print(failures == 0 ? "memory admission: PASS" : "memory admission: \(failures) FAILED")
exit(failures == 0 ? 0 : 1)
