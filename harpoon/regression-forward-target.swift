import Foundation

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fputs("FAIL: \(message)\n", stderr); exit(1) }
}

// --- which addresses may be published as the forwarder's guest target -------------------------
for ok in ["192.168.64.3", "192.168.64.254", "10.0.0.5", "172.16.0.1", "172.31.255.254", "192.168.0.1"] {
    require(isUsableForwardAddress(ok), "\(ok) is a usable private IPv4 address")
}
for bad in ["", " ", "127.0.0.1", "0.0.0.0", "255.255.255.255", "169.254.1.1", "224.0.0.1", "8.8.8.8", "172.15.0.1", "172.32.0.1",
            "192.169.1.1", "192.168.64", "192.168.64.3.4", "192.168.64.256", "192.168.064.3", "192.168.64.03", "192.168.64.-1",
            "192.168.64.3 ", " 192.168.64.3", "192.168.64.", ".168.64.3", "192.168.64.3\n", "::1", "fe80::1", "host.local", "1e2.168.64.3", "192.168.64.0x3"] {
    require(!isUsableForwardAddress(bad), "\(bad.debugDescription) must be refused")
}

// --- the published shape: a discovered address only, optional so old files and old readers are unaffected -----------
struct Meta: Codable { let pid: Int32; var forwardTarget: ForwardTarget? = nil }
let old = try! JSONDecoder().decode(Meta.self, from: Data(#"{"pid":7}"#.utf8))
require(old.forwardTarget == nil, "a runtime.json without the field decodes with no target")
let new = Meta(pid: 7, forwardTarget: ForwardTarget(address: "192.168.64.3", source: "discovered", recordedAt: "2026-11-04T00:00:00Z"))
let json = String(data: try! JSONEncoder().encode(new), encoding: .utf8)!
require(json.contains("\"forwardTarget\"") && json.contains("\"source\":\"discovered\"") && json.contains("192.168.64.3"), "the target is written with its source: \(json)")
let back = try! JSONDecoder().decode(Meta.self, from: Data(json.utf8))
require(back.forwardTarget == new.forwardTarget, "round trip")
let withdrawn = String(data: try! JSONEncoder().encode(Meta(pid: 7)), encoding: .utf8)!
require(!withdrawn.contains("forwardTarget"), "a withdrawn target leaves no key behind: \(withdrawn)")
// an old reader that ignores unknown keys still decodes a new file
struct OldMeta: Codable { let pid: Int32 }
require((try? JSONDecoder().decode(OldMeta.self, from: Data(json.utf8)))?.pid == 7, "an older reader ignores the new key")
print("forward target regression PASS")
