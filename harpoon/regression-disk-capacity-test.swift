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

// host space policy. Sparse growth allocates almost nothing now (metadata ~2% of the added capacity); the whole new size is only a
// worst case, limited to 4x the headroom (free - reserve) and flagged when it exceeds it.
let cap = 512 * GiB, reserve = 5 * GiB
func a(_ cur: UInt64, _ req: UInt64, phys: UInt64 = 2 * GiB, host: UInt64? = 100 * GiB, cap: UInt64 = cap) -> DiskPolicy.Assessment {
    DiskPolicy.assess(current: cur, physical: phys, requested: req, hostAvailable: host, maxBytes: cap, reserve: reserve)
}
func err(_ cur: UInt64, _ req: UInt64, phys: UInt64 = 2 * GiB, host: UInt64? = 100 * GiB, cap: UInt64 = cap) -> String? { a(cur, req, phys: phys, host: host, cap: cap).error }
require(err(4 * GiB, 8 * GiB) == nil, "4 -> 8 GiB with plenty of host space is allowed")
require(err(4 * GiB, 4 * GiB)?.contains("nothing to grow") == true, "the same size is refused")
require(err(8 * GiB, 4 * GiB)?.contains("shrinking is not supported") == true, "shrinking is refused")
require(err(32 * GiB, 8 * GiB)?.contains("shrinking is not supported") == true, "shrinking an existing 32 GiB disk is refused")
require(err(4 * GiB, 1 * GiB)?.contains("2 GiB minimum") == true, "below the minimum is refused")
require(err(4 * GiB, 600 * GiB)?.contains("growth cap") == true, "above the cap is refused")
require(err(4 * GiB, 128 * GiB, cap: 64 * GiB)?.contains("growth cap") == true, "a configured cap is enforced")
require(err(4 * GiB, 16 * GiB, host: nil)?.contains("could not be read") == true, "unknown host space refuses")
// the low-storage Mac: 19 GiB free, 2 GiB already allocated. 32 GiB is offered (flagged), not refused as the old rule did.
let lowMac = a(4 * GiB, 32 * GiB, phys: 2 * GiB, host: 19 * GiB)
require(lowMac.error == nil, "32 GiB on a 19 GiB-free Mac is allowed: only metadata is needed now")
require(lowMac.oversubscribed, "...and flagged: filling it needs 30 GiB, more than the 14 GiB headroom")
require(lowMac.immediateBytes == UInt64((Double(28 * GiB) * 0.02).rounded(.up)), "immediate need is 2% of the added capacity")
require(lowMac.worstCaseBytes == 30 * GiB, "worst case is the new size minus what is already allocated")
require(!a(4 * GiB, 8 * GiB, phys: 2 * GiB, host: 19 * GiB).oversubscribed, "8 GiB fits within the headroom: not flagged")
require(err(4 * GiB, 64 * GiB, phys: 2 * GiB, host: 19 * GiB)?.contains("4 times") == true, "62 GiB worst case exceeds 4 x 14 GiB: refused")
require(err(4 * GiB, 32 * GiB, phys: 2 * GiB, host: 8 * GiB) != nil, "32 GiB on 8 GiB free (headroom 3, limit 12) is refused")
require(err(4 * GiB, 8 * GiB, phys: 2 * GiB, host: 8 * GiB) == nil, "8 GiB on 8 GiB free is allowed (worst case 6 within 4 x 3)")
require(err(4 * GiB, 8 * GiB, host: 5 * GiB)?.contains("not enough host space") == true, "free space equal to the reserve leaves nothing for metadata: refused")
require(err(4 * GiB, 8 * GiB, host: 3 * GiB)?.contains("not enough host space") == true, "host below the reserve is refused")
require(err(4 * GiB, 8 * GiB, phys: 8 * GiB, host: reserve + UInt64((Double(4 * GiB) * 0.02).rounded(.up)) - 1)?.contains("not enough host space") == true, "one byte short of metadata + reserve is refused")
require(err(4 * GiB, 8 * GiB, phys: 8 * GiB, host: reserve + UInt64((Double(4 * GiB) * 0.02).rounded(.up))) == nil, "exactly metadata + reserve is allowed")
// a large allocation already on the host raises the worst case only by the unallocated part
require(a(100 * GiB, 128 * GiB, phys: 90 * GiB, host: 40 * GiB).error == nil, "a mostly allocated disk only needs its unallocated remainder in the worst case")

// suggestions: only larger, only what would be accepted, at most three, with the oversubscription flag
func sug(_ cur: UInt64, phys: UInt64 = 2 * GiB, host: UInt64? = 500 * GiB, cap: UInt64 = cap) -> [(bytes: UInt64, oversubscribed: Bool)] {
    DiskPolicy.suggestions(current: cur, physical: phys, hostAvailable: host, maxBytes: cap, reserve: reserve)
}
func sizes(_ x: [(bytes: UInt64, oversubscribed: Bool)]) -> [UInt64] { x.map { $0.bytes } }
require(sizes(sug(4 * GiB)) == [8 * GiB, 16 * GiB, 32 * GiB], "from 4 GiB: 8, 16, 32")
require(sizes(sug(32 * GiB)) == [64 * GiB, 128 * GiB, 256 * GiB], "an existing 32 GiB disk is offered larger sizes only")
require(sizes(sug(6 * GiB)) == [8 * GiB, 16 * GiB, 32 * GiB], "a custom 6 GiB disk is offered the next ladder steps")
require(sug(512 * GiB).isEmpty, "at the cap nothing is offered")
require(sizes(sug(4 * GiB, cap: 16 * GiB)) == [8 * GiB, 16 * GiB], "the cap trims the offer")
require(sug(4 * GiB, host: 4 * GiB).isEmpty, "a host with less than the reserve is offered nothing")
require(sug(4 * GiB, host: nil).isEmpty, "unknown host space is offered nothing")
require(sug(4 * GiB).allSatisfy { $0.bytes > 4 * GiB && !$0.oversubscribed }, "plenty of host space: larger sizes, none flagged")
let low = sug(4 * GiB, phys: 2 * GiB, host: 19 * GiB)
require(sizes(low) == [8 * GiB, 16 * GiB, 32 * GiB], "a 19 GiB-free Mac is still offered 8, 16 and 32 GiB")
require(low.map { $0.oversubscribed } == [false, false, true], "...with only 32 GiB flagged")

// filesystem verification after the restart: absolute, so an unknown earlier reading cannot fake success
require(DiskPolicy.filesystemExpanded(after: 7_900 * MiB, from: 4 * GiB, to: 8 * GiB), "a filesystem that reports about the new size is expanded")
require(!DiskPolicy.filesystemExpanded(after: 3_900 * MiB, from: 4 * GiB, to: 8 * GiB), "an unchanged filesystem is not expanded")
require(!DiskPolicy.filesystemExpanded(after: 5_000 * MiB, from: 4 * GiB, to: 8 * GiB), "a partly grown filesystem is not expanded")
require(!DiskPolicy.filesystemExpanded(after: 0, from: 4 * GiB, to: 8 * GiB), "a zero reading is not expanded")
require(!DiskPolicy.filesystemExpanded(after: 7 * GiB, from: 8 * GiB, to: 4 * GiB), "a shrink is never an expansion")

// interrupted-expansion record: durable, read back, explained from the disk's actual size
let rec = base + "/rec/disk-expansion.json"
let r = DiskExpansionRecord(phase: "growing", fromBytes: 4 * GiB, toBytes: 8 * GiB, diskPath: "/x/harpoon-root.img", startedAt: "t", pid: getpid())
require(DiskExpansionRecord.load(at: rec) == nil, "no record before an expansion")
require(r.save(at: rec) == nil, "a record saves (creating its directory)")
require(DiskExpansionRecord.load(at: rec) == r, "the record round-trips")
require((try! fm.attributesOfItem(atPath: rec))[.posixPermissions] as! Int == 0o600, "the record is private")
require(!fm.fileExists(atPath: rec + ".tmp"), "no temp file is left behind")
var r2 = r; r2.phase = "starting"
require(r2.save(at: rec) == nil && DiskExpansionRecord.load(at: rec)?.phase == "starting", "an update replaces the record")
// a record that cannot be saved says so: a directory in its place, an unwritable directory
try! fm.createDirectory(atPath: base + "/blocked/disk-expansion.json", withIntermediateDirectories: true)
require(r.save(at: base + "/blocked/disk-expansion.json") != nil, "a record path that is a directory is a save failure")
try! fm.createDirectory(atPath: base + "/ro", withIntermediateDirectories: true)
chmod(base + "/ro", 0o500)
if geteuid() != 0 { require(r.save(at: base + "/ro/disk-expansion.json") != nil, "an unwritable directory is a save failure") }
chmod(base + "/ro", 0o700)
require(DiskExpansionRecord.load(at: base + "/ro/disk-expansion.json") == nil, "a failed save leaves no record")
// recovery is judged from the disk file, whatever phase was last written
for phase in ["stopping", "growing", "starting", "verifying"] {
    var x = r; x.phase = phase
    let notGrown = x.recovery(currentLogical: 4 * GiB), grown = x.recovery(currentLogical: 8 * GiB)
    require(notGrown.contains("was not grown") && notGrown.contains("harpoon disk expand 8G --yes"), "\(phase), not grown: retry the expansion")
    require(grown.contains("already 8 GiB") && grown.contains("harpoon start") && grown.contains("expands the filesystem on boot"), "\(phase), grown: start and the guest finishes")
    require(!notGrown.lowercased().contains("rolled back") && !grown.lowercased().contains("rolled back"), "no rollback is claimed")
}
require(r.isActive == isHarpoonProcess(pid: getpid()), "a record is active exactly when its live process is a harpoon process")
var dead = r; dead.pid = 99999999
require(!dead.isActive, "a record whose process is gone is interrupted, not active")
var nopid = r; nopid.pid = nil
require(!nopid.isActive, "a record without a pid is never active")
DiskExpansionRecord.clear(at: rec)
require(DiskExpansionRecord.load(at: rec) == nil, "the record is removed when cleared")

// the restart must reproduce the running VM exactly
func meta(cpus: Int = 2, mem: Int = 2048, disk: String, kernel: String? = "K", initramfs: String? = "I") -> RuntimeMetadata {
    RuntimeMetadata(pid: 1, startedAt: "t", cpus: cpus, memoryMiB: mem, diskPath: disk, socketPath: "s", uuid: "u", binary: "b",
                    diskIdentity: nil, diskResolvedPath: nil, failure: nil, kernelPath: kernel, initramfsPath: initramfs)
}
let diskA = base + "/a.img", diskB = base + "/b.img", kA = base + "/kernel", iA = base + "/initramfs", kB = base + "/kernel2"
for f in [diskA, diskB, kA, iA, kB] { fm.createFile(atPath: f, contents: Data("x".utf8)) }
func cfg(cpus: Int = 2, mem: Int = 2048, disk: String, kernel: String, initramfs: String) -> RuntimeConfig {
    var c = RuntimeConfig(); c.cpuCount = cpus; c.memoryMIB = mem; c.diskURL = URL(fileURLWithPath: disk)
    c.kernelURL = URL(fileURLWithPath: kernel); c.initramfsURL = URL(fileURLWithPath: initramfs); return c
}
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskA, kernel: kA, initramfs: iA)).isEmpty, "identical settings: nothing to refuse")
require(expansionConfigMismatches(meta: meta(cpus: 4, disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskA, kernel: kA, initramfs: iA)).contains { $0.hasPrefix("cpus") }, "different CPUs are refused")
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(mem: 4096, disk: diskA, kernel: kA, initramfs: iA)).contains { $0.hasPrefix("memory") }, "different memory is refused")
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskB, kernel: kA, initramfs: iA)).contains { $0.hasPrefix("disk") }, "a different disk is refused")
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskA, kernel: kB, initramfs: iA)).contains { $0.hasPrefix("kernel") }, "a different kernel is refused")
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskA, kernel: kA, initramfs: kB)).contains { $0.hasPrefix("initramfs") }, "a different initramfs is refused")
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: nil, initramfs: nil), resolved: cfg(disk: diskA, kernel: kA, initramfs: iA)).count == 2, "a VM started by an older build (kernel/initramfs unrecorded) cannot be guaranteed")
try! fm.createSymbolicLink(atPath: base + "/k-link", withDestinationPath: kA)
require(expansionConfigMismatches(meta: meta(disk: diskA, kernel: kA, initramfs: iA), resolved: cfg(disk: diskA, kernel: base + "/k-link", initramfs: iA)).isEmpty, "a symlink to the same kernel is the same kernel")

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
