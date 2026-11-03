import Foundation

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
}

// --- route selection from the address Docker reports -----------------------------------------
// Ordinary publishing (-p 8080:80): Docker binds the guest's wildcard, reachable on the guest address.
for ip in [nil, "", "  ", "0.0.0.0", "::", "0.0.0.0/0"] as [String?] {
    require(PublishedPortRoute(dockerHostIP: ip) == .guestAddress, "ordinary HostIp \(String(describing: ip)) uses the guest address")
}
// Explicit IPv4 loopback (-p 127.0.0.1:8080:80): bound on guest loopback only, so it needs the relay.
require(PublishedPortRoute(dockerHostIP: "127.0.0.1") == .guestLoopback(ip: "127.0.0.1"), "127.0.0.1 is guest loopback")
require(PublishedPortRoute(dockerHostIP: " 127.0.0.1 ") == .guestLoopback(ip: "127.0.0.1"), "whitespace is trimmed")
require(PublishedPortRoute(dockerHostIP: "127.0.0.2") == .guestLoopback(ip: "127.0.0.2"), "any 127/8 address is guest loopback, kept exact")
// Near misses must NOT be treated as loopback (they would be relayed to the wrong place).
for ip in ["127.1", "127.0.0.1x", "1270.0.0.1", "128.0.0.1", "126.255.255.255", "192.168.64.3", "localhost", "127.0.0.1; reboot", "127.0.0.1:80"] {
    require(PublishedPortRoute(dockerHostIP: ip) == .guestAddress, "\(ip) is not loopback")
}
// IPv6 loopback is out of scope: reported, never silently relayed to the guest address.
if case .unsupported(let reason) = PublishedPortRoute(dockerHostIP: "::1") {
    require(reason.contains("IPv6"), "::1 reason names IPv6")
} else {
    require(false, "::1 must be unsupported")
}

// --- vsock port and guest command shape --------------------------------------------------------
require(GuestRelay.vsockPort(hostPort: 1) > 65535, "relay vsock port is outside the TCP range")
require(GuestRelay.vsockPort(hostPort: 65535) == 65536 + 65535, "relay vsock port is base + host port")
for fixed: UInt32 in [2375, 2377] {
    for p in 1...UInt16.max { require(GuestRelay.vsockPort(hostPort: p) != fixed, "relay vsock port never equals \(fixed)") }
}
let start = GuestRelay.startArgv(hostPort: 43871, loopbackIP: "127.0.0.1")
require(start == ["sh", "-c", GuestRelay.startScript, "harpoon-relay", "109407", "127.0.0.1", "43871"], "start argv passes values as positional parameters, never interpolated into the script")
require(!GuestRelay.startScript.contains("43871") && !GuestRelay.startScript.contains("127.0.0.1"), "start script has no baked-in port or address")
require(GuestRelay.startScript.contains("backlog=128"), "relay listener accepts a burst of connections (socat's default backlog resets them)")
require(GuestRelay.startScript.contains("TCP4:$2:$3"), "relay connects only to the given IPv4 loopback address")
require(GuestRelay.startScript.contains("[V]SOCK-LISTEN:$1,") && GuestRelay.stopScript.contains("[V]SOCK-LISTEN:$1,"), "pkill patterns cannot match the shell carrying them")

// --- the guest scripts, executed for real against shimmed guest tools ----------------------------
// macOS has no setsid and no guest socat; the shims keep the process-management logic (pkill/pgrep/sleep/sh)
// real. The fake socat takes the real relay's command line, so the real patterns must find and kill it.
let shims = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("harpoon-relay-shims-\(UUID().uuidString)")
try FileManager.default.createDirectory(at: shims, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: shims) }
func shim(_ name: String, _ body: String) throws {
    let url = shims.appendingPathComponent(name)
    try ("#!/bin/bash\n" + body + "\n").write(to: url, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
}
try shim("setsid", "exec \"$@\"")
try shim("socat", "[ -n \"$SHIM_SOCAT_FAIL\" ] && exit 1\nexec -a \"socat $1 $2\" /bin/sleep 30")
let env = ProcessInfo.processInfo.environment.merging(["PATH": shims.path + ":/usr/bin:/bin"]) { _, new in new }

@discardableResult
func run(_ argv: [String], extraEnv: [String: String] = [:]) -> (code: Int32, err: String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    p.arguments = argv
    p.environment = env.merging(extraEnv) { _, new in new }
    let e = Pipe(); p.standardError = e; p.standardOutput = FileHandle.nullDevice
    try! p.run(); p.waitUntilExit()
    return (p.terminationStatus, String(decoding: e.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self))
}
func relayCount(_ hostPort: UInt16) -> Int {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
    p.arguments = ["-f", "[V]SOCK-LISTEN:\(GuestRelay.vsockPort(hostPort: hostPort)),"]
    let o = Pipe(); p.standardOutput = o
    try! p.run(); p.waitUntilExit()
    return String(decoding: o.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).split(separator: "\n").count
}
let a: UInt16 = 41231, b: UInt16 = 41232
defer { run(GuestRelay.stopArgv(hostPort: a)); run(GuestRelay.stopArgv(hostPort: b)) }

require(relayCount(a) == 0, "no relay before start")
require(run(GuestRelay.startArgv(hostPort: a, loopbackIP: "127.0.0.1")).code == 0, "start succeeds")
require(relayCount(a) == 1, "start leaves exactly one relay running")
require(run(GuestRelay.startArgv(hostPort: a, loopbackIP: "127.0.0.1")).code == 0, "start is idempotent")
require(relayCount(a) == 1, "restart replaces the relay instead of stacking a second one")
require(run(GuestRelay.startArgv(hostPort: b, loopbackIP: "127.0.0.2")).code == 0, "second port starts")
require(relayCount(a) == 1 && relayCount(b) == 1, "relays are independent")
require(run(GuestRelay.stopArgv(hostPort: a)).code == 0, "stop succeeds")
require(relayCount(a) == 0, "stop removes that port's relay")
require(relayCount(b) == 1, "stop leaves other ports' relays alone")
require(run(GuestRelay.stopArgv(hostPort: a)).code == 0, "stopping an absent relay is not an error")
require(run(GuestRelay.stopArgv(hostPort: b)).code == 0 && relayCount(b) == 0, "second stop")
let failed = run(GuestRelay.startArgv(hostPort: a, loopbackIP: "127.0.0.1"), extraEnv: ["SHIM_SOCAT_FAIL": "1"])
require(failed.code != 0 && failed.err.contains("guest relay did not start"), "a relay that dies at start is reported, with a reason")
require(relayCount(a) == 0, "a failed start leaves nothing behind")

print("published-port route regression PASS")
