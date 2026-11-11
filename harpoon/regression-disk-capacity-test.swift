import Foundation

// Unit test for the disk capacity policy (DiskCapacity.swift): low-space thresholds, suggested sizes, the refusal rules of an expansion
// (shrink, cap, host reserve), the interrupted-expansion record, and protection of the immutable guest template. Pure numbers and
// scratch files under HARPOON_TEST_TMPDIR; no VM, no engine, nothing is stopped or resized. `log` is main.swift's.
func log(_ m: String) {}
func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fputs("FAIL: \(message)\n", stderr); exit(1) }
}
let fm = FileManager.default
let GiB: UInt64 = 1024 * 1024 * 1024
let MiB: UInt64 = 1024 * 1024
let base = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"]!

// default provisioning policy: 4 GiB, provisional; the minimum is still 2 GiB
require(RuntimeConfig.defaultProvisionBytes == 4 * GiB, "the default new disk is 4 GiB")
require(RuntimeConfig.minProvisionBytes == 2 * GiB, "the minimum disk is 2 GiB")

// thresholds: percentage OR absolute free space, so small and large disks both warn in time
func p(_ used: UInt64, _ avail: UInt64) -> DiskPressure { DiskPolicy.pressure(used: used, available: avail) }
require(p(400 * MiB, 3_400 * MiB) == .ok, "a fresh 4 GiB disk (about 10% used) is fine")
require(p(2 * GiB, 2 * GiB) == .ok, "50% of 4 GiB with 2 GiB free is fine")
require(p(3 * GiB, 1 * GiB) == .warning, "4 GiB disk with 1 GiB free warns on absolute space (75% used)")
require(p(3_300 * MiB, 800 * MiB) == .warning, "about 80% of 4 GiB warns")
require(p(3_800 * MiB, 300 * MiB) == .critical, "300 MiB free is critical")
require(p(26 * GiB, 6 * GiB) == .warning, "81% of 32 GiB warns on percentage though 6 GiB is free")
require(p(20 * GiB, 12 * GiB) == .ok, "62% of 32 GiB with 12 GiB free is fine")
require(p(30 * GiB, 2 * GiB) == .critical, "94% of 32 GiB is critical")
require(p(200 * GiB, 300 * GiB) == .ok, "a large disk with plenty free is fine")
require(p(0, 0) == .unknown, "no numbers is unknown, not ok")
// warning comes before critical and the thresholds are ordered
require(DiskPolicy.warnUsedPercent < DiskPolicy.criticalUsedPercent && DiskPolicy.warnAvailableBytes > DiskPolicy.criticalAvailableBytes, "warning precedes critical")

// configured limits
require(DiskPolicy.maxBytes(environment: nil) == 512 * GiB, "default growth cap is 512 GiB")
require(DiskPolicy.maxBytes(environment: "64G") == 64 * GiB, "HARPOON_DISK_MAX is honoured")
require(DiskPolicy.maxBytes(environment: "banana") == 512 * GiB, "an unparseable cap falls back to the default, not to unlimited")
require(DiskPolicy.maxBytes(environment: "1G") == 512 * GiB, "a cap below the minimum disk is ignored")
require(DiskPolicy.hostReserveBytes(environment: nil) == 5 * GiB, "default host reserve is 5 GiB")
require(DiskPolicy.hostReserveBytes(environment: "1G") == 1 * GiB, "HARPOON_DISK_HOST_RESERVE is honoured")
require(DiskPolicy.hostReserveBytes(environment: "banana") == 5 * GiB, "an unparseable reserve keeps the default")

// expansion rules
let cap = 512 * GiB, reserve = 5 * GiB
func err(_ cur: UInt64, _ req: UInt64, host: UInt64? = 100 * GiB, cap: UInt64 = cap) -> String? {
    DiskPolicy.expansionError(current: cur, requested: req, hostAvailable: host, maxBytes: cap, reserve: reserve)
}
require(err(4 * GiB, 8 * GiB) == nil, "4 -> 8 GiB with plenty of host space is allowed")
require(err(4 * GiB, 4 * GiB)?.contains("nothing to grow") == true, "the same size is refused")
require(err(8 * GiB, 4 * GiB)?.contains("shrinking is not supported") == true, "shrinking is refused")
require(err(32 * GiB, 8 * GiB)?.contains("shrinking is not supported") == true, "shrinking an existing 32 GiB disk is refused")
require(err(4 * GiB, 1 * GiB)?.contains("2 GiB minimum") == true, "below the minimum is refused")
require(err(4 * GiB, 600 * GiB)?.contains("growth cap") == true, "above the cap is refused")
require(err(4 * GiB, 128 * GiB, cap: 64 * GiB)?.contains("growth cap") == true, "a configured cap is enforced")
require(err(4 * GiB, 16 * GiB, host: 16 * GiB)?.contains("not enough host space") == true, "host space equal to the growth leaves no reserve: refused")
require(err(4 * GiB, 16 * GiB, host: 17 * GiB - 1)?.contains("not enough host space") == true, "12 GiB growth + 5 GiB reserve needs 17 GiB: one byte less is refused")
require(err(4 * GiB, 16 * GiB, host: 12 * GiB + reserve) == nil, "exactly growth + reserve is allowed")
require(err(4 * GiB, 16 * GiB, host: nil)?.contains("could not be read") == true, "unknown host space refuses")
require(err(4 * GiB, 16 * GiB, host: 2 * GiB)?.contains("not enough host space") == true, "host smaller than the growth is refused")

// suggestions: only larger, only what would be accepted, at most three
func sug(_ cur: UInt64, host: UInt64? = 500 * GiB, cap: UInt64 = cap) -> [UInt64] {
    DiskPolicy.suggestedSizes(current: cur, hostAvailable: host, maxBytes: cap, reserve: reserve)
}
require(sug(4 * GiB) == [8 * GiB, 16 * GiB, 32 * GiB], "from 4 GiB: 8, 16, 32")
require(sug(32 * GiB) == [64 * GiB, 128 * GiB, 256 * GiB], "an existing 32 GiB disk is offered larger sizes only")
require(sug(6 * GiB) == [8 * GiB, 16 * GiB, 32 * GiB], "a custom 6 GiB disk is offered the next ladder steps")
require(sug(512 * GiB).isEmpty, "at the cap nothing is offered")
require(sug(4 * GiB, cap: 16 * GiB) == [8 * GiB, 16 * GiB], "the cap trims the offer")
require(sug(4 * GiB, host: 20 * GiB) == [8 * GiB, 16 * GiB], "host space trims the offer (16 needs 12+5 <= 20; 32 does not)")
require(sug(4 * GiB, host: 6 * GiB).isEmpty, "a nearly full host is offered nothing")
require(sug(4 * GiB, host: nil).isEmpty, "unknown host space is offered nothing")
require(sug(4 * GiB).allSatisfy { $0 > 4 * GiB }, "every suggestion is larger than the current size")

// filesystem verification after the restart
require(DiskPolicy.filesystemExpanded(before: 3_900 * MiB, after: 7_900 * MiB, from: 4 * GiB, to: 8 * GiB), "a filesystem that gained about the added capacity is expanded")
require(!DiskPolicy.filesystemExpanded(before: 3_900 * MiB, after: 3_900 * MiB, from: 4 * GiB, to: 8 * GiB), "an unchanged filesystem is not expanded")
require(!DiskPolicy.filesystemExpanded(before: 3_900 * MiB, after: 5_000 * MiB, from: 4 * GiB, to: 8 * GiB), "a partly grown filesystem is not expanded")
require(!DiskPolicy.filesystemExpanded(before: 0, after: 7 * GiB, from: 8 * GiB, to: 4 * GiB), "a shrink is never an expansion")

// interrupted-expansion record: written before the work, removed on success, explains each interruption
let rec = base + "/disk-expansion.json"
let r = DiskExpansionRecord(phase: "growing", fromBytes: 4 * GiB, toBytes: 8 * GiB, diskPath: "/x/harpoon-root.img", startedAt: "t")
require(DiskExpansionRecord.load(at: rec) == nil, "no record before an expansion")
r.save(at: rec)
require(DiskExpansionRecord.load(at: rec) == r, "the record round-trips")
require(r.recovery(currentLogical: 4 * GiB).contains("was not grown") && r.recovery(currentLogical: 4 * GiB).contains("harpoon disk expand 8G --yes"), "growing, not yet grown: retry the expansion")
require(r.recovery(currentLogical: 8 * GiB).contains("harpoon start") && r.recovery(currentLogical: 8 * GiB).contains("expands the filesystem on boot"), "growing, already grown: start and the guest finishes")
var s1 = r; s1.phase = "stopping"
require(s1.recovery(currentLogical: 4 * GiB).contains("disk was not changed"), "interrupted while stopping: the disk is unchanged")
var s2 = r; s2.phase = "starting"
require(s2.recovery(currentLogical: 8 * GiB).contains("harpoon start"), "interrupted while starting: start it")
require(!s2.recovery(currentLogical: 8 * GiB).lowercased().contains("rolled back"), "no rollback is claimed")
DiskExpansionRecord.clear(at: rec)
require(DiskExpansionRecord.load(at: rec) == nil, "the record is removed when cleared")

// the immutable template is never a disk
let tmplDir = base + "/tmpl"
try! fm.createDirectory(atPath: tmplDir, withIntermediateDirectories: true)
let tmpl = tmplDir + "/harpoon-root.img"
fm.createFile(atPath: tmpl, contents: Data(repeating: 1, count: 4096))
setenv("HARPOON_TEST_ROOT", tmpl, 1)
setenv("HARPOON_TEST_TMPDIR", base + "/ns", 1)
require(RuntimeConfig.templateProtectionError(for: tmpl) != nil, "the template path itself is refused")
try! fm.createDirectory(atPath: base + "/ns/data", withIntermediateDirectories: true)
try! fm.createSymbolicLink(atPath: base + "/ns/data/via-symlink.img", withDestinationPath: tmpl)
require(RuntimeConfig.templateProtectionError(for: base + "/ns/data/via-symlink.img") != nil, "a symlink to the template is refused")
try! fm.linkItem(atPath: tmpl, toPath: base + "/ns/data/via-hardlink.img")
require(RuntimeConfig.templateProtectionError(for: base + "/ns/data/via-hardlink.img") != nil, "a hard link to the template is refused (same inode)")
let other = base + "/ns/data/harpoon-root.img"
fm.createFile(atPath: other, contents: Data(repeating: 2, count: 4096))
require(RuntimeConfig.templateProtectionError(for: other) == nil, "an ordinary disk is allowed")
require(growable(other), "an ordinary disk validates") 
func growable(_ path: String) -> Bool { var c = RuntimeConfig(); c.diskURL = URL(fileURLWithPath: path); return c.templateProtectionError == nil }
extension RuntimeConfig { var templateProtectionError: String? { Self.templateProtectionError(for: diskURL.path) } }
let t0 = (try! fm.attributesOfItem(atPath: tmpl))[.size] as! UInt64
require(t0 == 4096, "the template was not modified by any check")
print("disk capacity unit test PASS")
